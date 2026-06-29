//! The provider runtime.
//!
//! A provider process hosts the `#[napi]` functions. It connects to (or listens
//! for) the Node peer over a named socket, performs the [`Hello`] handshake
//! advertising its [`registered_names`], then services [`Request`]s by routing
//! them through [`dispatch`] until the peer disconnects.
//!
//! This module is pure library surface: it exposes the serve/connect primitives
//! an application's `fn main` calls. The entrypoint itself (CLI parsing, process
//! exit, which side is parent) is the application's concern — see the
//! `add-numbers` example's `main.rs`.

use std::io;
use std::process::Command;

use interprocess::local_socket::Stream;
use interprocess::TryClone;

use crate::bootstrap::{cleanup_socket_path, generate_socket_path, SOCKET_ENV};
use crate::codec::{read_message, write_message, Hello, Message, Role};
use crate::peer::handshake;
use crate::registry::{dispatch, registered_names};
use crate::transport::{connect, listen};
use crate::PROTOCOL_VERSION;

/// The [`Hello`] this provider announces: its protocol version, role, and the
/// names of every registered `#[napi]` function.
pub fn provider_hello() -> Hello {
    Hello {
        version: PROTOCOL_VERSION,
        role: Role::Provider,
        functions: registered_names(),
    }
}

/// Handshake, then serve requests on a connected stream until the peer closes.
///
/// Requests are dispatched **concurrently**: each runs on its own thread, so a
/// slow (e.g. `async`) call doesn't head-of-line-block others. The stream is
/// cloned into independent read and write handles (full-duplex), with the writer
/// shared behind a mutex so messages serialize. Replies may complete out of
/// order, matched by correlation id on the caller side.
///
/// A function may invoke callbacks the peer passed as arguments. Like napi's
/// `ThreadsafeFunction`, that is fire-and-forget: the dispatch thread writes a
/// `CallbackInvoke` and continues — the peer runs it on its event loop.
pub fn serve(mut stream: Stream) -> io::Result<()> {
    handshake(&mut stream, provider_hello())?;
    let writer = std::sync::Arc::new(std::sync::Mutex::new(stream.try_clone()?));
    let callbacks: std::sync::Arc<dyn crate::registry::Callbacks> =
        std::sync::Arc::new(ProviderCallbacks { writer: std::sync::Arc::clone(&writer) });

    // A small fixed pool reads requests off a channel, so threads are reused
    // across calls rather than spawned per request, and don't grow unboundedly.
    // Replies are matched by correlation id, so out-of-order completion is fine.
    let (tx, rx) = std::sync::mpsc::channel::<crate::codec::Request>();
    let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
    let pool: Vec<_> = (0..worker_count())
        .map(|_| {
            let rx = std::sync::Arc::clone(&rx);
            let writer = std::sync::Arc::clone(&writer);
            let callbacks = std::sync::Arc::clone(&callbacks);
            std::thread::spawn(move || loop {
                let request = match rx.lock().unwrap().recv() {
                    Ok(r) => r,
                    Err(_) => break, // sender dropped: connection closed
                };
                let reply = dispatch(request, &callbacks);
                let _ = write_message(&mut *writer.lock().unwrap(), &reply);
            })
        })
        .collect();

    let mut reader = stream;
    loop {
        match read_message(&mut reader)? {
            None => break,
            Some(Message::Request(request)) => {
                if tx.send(request).is_err() {
                    break;
                }
            }
            Some(Message::ReleaseExternal(r)) => crate::types::release_external(r.token),
            Some(_other) => {}
        }
    }
    drop(tx);
    for w in pool {
        let _ = w.join();
    }
    Ok(())
}

/// Size of the request worker pool: available parallelism, min 1.
fn worker_count() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
}

/// The [`Callbacks`] impl handed to each dispatched function: fire-and-forget,
/// writing a `CallbackInvoke` and returning immediately. Holds an owned writer
/// so a stored `ThreadsafeFunction` can keep firing after the call returns.
struct ProviderCallbacks {
    writer: std::sync::Arc<std::sync::Mutex<Stream>>,
}

impl crate::registry::Callbacks for ProviderCallbacks {
    fn invoke(&self, handle: u64, args: Vec<rmpv::Value>) {
        let msg = Message::CallbackInvoke(crate::codec::CallbackInvoke { handle, args });
        let _ = write_message(&mut *self.writer.lock().unwrap(), &msg);
    }

    fn release(&self, handle: u64) {
        let msg = Message::Release(crate::codec::Release { handle });
        let _ = write_message(&mut *self.writer.lock().unwrap(), &msg);
    }
}

/// Connect to a peer listening at `path` and serve it.
pub fn connect_and_serve(path: &str) -> io::Result<()> {
    serve(connect(path)?)
}

/// Listen at `path`, accept one peer, and serve it.
pub fn listen_and_serve(path: &str) -> io::Result<()> {
    let listener = listen(path)?;
    serve(listener.accept()?)
}

/// Serve as the **child**: read the socket path the parent exported in
/// [`SOCKET_ENV`], connect to it, and serve. Used when another process (Node or
/// Rust) spawned us.
pub fn serve_from_env() -> io::Result<()> {
    let path = std::env::var(SOCKET_ENV).map_err(|_| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("{SOCKET_ENV} not set; expected to be spawned as a child"),
        )
    })?;
    connect_and_serve(&path)
}

/// Serve as the **parent**: generate a socket path, bind a listener, export the
/// path to `command` via [`SOCKET_ENV`], spawn it as the child peer, then accept
/// and serve the one connection. Waits for the child and cleans up the socket
/// file before returning the serve result.
///
/// The caller configures `command` (program + args); this only injects the
/// socket-path environment variable, keeping process policy in the application.
pub fn spawn_and_serve(mut command: Command) -> io::Result<()> {
    let path = generate_socket_path();
    let listener = listen(&path)?;
    command.env(SOCKET_ENV, &path);

    let mut child = command.spawn()?;
    let result = serve(listener.accept()?);

    let _ = child.wait();
    cleanup_socket_path(&path);
    result
}
