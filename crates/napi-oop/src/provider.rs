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
/// shared behind a mutex so responses serialize. Replies may complete out of
/// order, matched by correlation id on the caller side.
///
/// A function may invoke callbacks the peer passed as arguments: the dispatch
/// thread sends a `CallbackInvoke` and blocks on the matching `CallbackResult`,
/// which the read loop routes back via a per-call channel.
pub fn serve(mut stream: Stream) -> io::Result<()> {
    handshake(&mut stream, provider_hello())?;
    let writer = std::sync::Arc::new(std::sync::Mutex::new(stream.try_clone()?));
    let pending = std::sync::Arc::new(CallbackPending::default());
    let mut reader = stream;
    let mut workers = Vec::new();
    loop {
        match read_message(&mut reader)? {
            None => break,
            Some(Message::Request(request)) => {
                let writer = std::sync::Arc::clone(&writer);
                let pending = std::sync::Arc::clone(&pending);
                workers.push(std::thread::spawn(move || {
                    let cb = ProviderCallbacks { writer: &writer, pending: &pending };
                    let reply = dispatch(request, &cb);
                    let _ = write_message(&mut *writer.lock().unwrap(), &reply);
                }));
            }
            // A callback result arrived: route it to the blocked dispatch thread.
            Some(Message::CallbackResult(r)) => pending.complete(r.id, Ok(r.result)),
            Some(_other) => {}
        }
    }
    for w in workers {
        let _ = w.join();
    }
    Ok(())
}

/// Outstanding callback invocations, awaiting a `CallbackResult` from the peer.
#[derive(Default)]
struct CallbackPending {
    next: std::sync::atomic::AtomicU64,
    map: std::sync::Mutex<std::collections::HashMap<u64, std::sync::mpsc::Sender<Result<rmpv::Value, String>>>>,
}

impl CallbackPending {
    fn register(&self) -> (u64, std::sync::mpsc::Receiver<Result<rmpv::Value, String>>) {
        let id = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (tx, rx) = std::sync::mpsc::channel();
        self.map.lock().unwrap().insert(id, tx);
        (id, rx)
    }
    fn complete(&self, id: u64, value: Result<rmpv::Value, String>) {
        if let Some(tx) = self.map.lock().unwrap().remove(&id) {
            let _ = tx.send(value);
        }
    }
}

/// The [`Callbacks`] impl handed to each dispatched function: sends a
/// `CallbackInvoke` and blocks for the matching result.
struct ProviderCallbacks<'a> {
    writer: &'a std::sync::Mutex<Stream>,
    pending: &'a CallbackPending,
}

impl crate::registry::Callbacks for ProviderCallbacks<'_> {
    fn invoke(&self, handle: u64, args: Vec<rmpv::Value>) -> Result<rmpv::Value, String> {
        let (id, rx) = self.pending.register();
        let msg = Message::CallbackInvoke(crate::codec::CallbackInvoke { id, handle, args });
        write_message(&mut *self.writer.lock().unwrap(), &msg).map_err(|e| e.to_string())?;
        rx.recv().map_err(|_| "callback channel closed".to_string())?
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
