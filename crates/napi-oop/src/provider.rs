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

use std::io::{self, Read, Write};
use std::process::Command;

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
pub fn serve<S: Read + Write>(mut stream: S) -> io::Result<()> {
    handshake(&mut stream, provider_hello())?;
    loop {
        match read_message(&mut stream)? {
            None => return Ok(()),
            Some(Message::Request(request)) => {
                let reply = dispatch(request);
                write_message(&mut stream, &reply)?;
            }
            // Non-request traffic (callbacks etc.) is added in a later phase.
            Some(_other) => {}
        }
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
