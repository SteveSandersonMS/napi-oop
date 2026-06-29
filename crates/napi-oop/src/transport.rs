//! The duplex byte channel between the two peers.
//!
//! Implemented as a **named local socket**: a Unix domain socket on Linux/macOS
//! and a named pipe on Windows, via the `interprocess` crate. stdio is
//! intentionally never used, since the child process may need stdout/stderr for
//! its own output.
//!
//! The bootstrap (who generates the path, who spawns whom) is symmetric and is
//! handled in a later phase; here we just listen/connect at a given path.

use std::io::{self, Read, Write};

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{
    GenericFilePath, Listener, ListenerNonblockingMode, ListenerOptions, Stream,
};

/// A bidirectional, ordered byte stream connecting the Node and Rust peers.
///
/// Any `Read + Write + Send` type qualifies, so tests can use in-memory pipes
/// and production uses the named-socket [`Stream`].
pub trait Transport: Read + Write + Send {}

impl<T: Read + Write + Send> Transport for T {}

/// A listening named local socket. The parent side binds this and accepts the
/// child's connection.
pub struct NamedSocketListener {
    inner: Listener,
}

impl NamedSocketListener {
    /// Block until a peer connects, returning the connected stream.
    pub fn accept(&self) -> io::Result<Stream> {
        let stream = self.inner.accept()?;
        // On BSD/macOS an accepted socket inherits the listener's non-blocking
        // flag, and `interprocess` only ever *enables* non-blocking on accepted
        // streams, never disables it (see its `accept` TODO). When we put the
        // listener in non-blocking accept mode to poll for a connection, that
        // flag would otherwise leak onto the connection and make the serve loop's
        // blocking reads fail with `WouldBlock`/`EAGAIN`. Force the stream back to
        // blocking; a no-op where it already is (Linux/Windows).
        stream.set_nonblocking(false)?;
        Ok(stream)
    }

    /// Toggle non-blocking `accept`: when enabled, [`accept`](Self::accept)
    /// returns a [`WouldBlock`](io::ErrorKind::WouldBlock) error instead of
    /// parking when no peer is currently connecting. Accepted streams stay
    /// blocking. Used to poll for a connection while also watching for a child
    /// that may exit before it connects.
    pub fn set_nonblocking_accept(&self, nonblocking: bool) -> io::Result<()> {
        let mode = if nonblocking {
            ListenerNonblockingMode::Accept
        } else {
            ListenerNonblockingMode::Neither
        };
        self.inner.set_nonblocking(mode)
    }
}

/// Bind a listener at the given filesystem socket path (Unix) / pipe path
/// (Windows).
pub fn listen(path: &str) -> io::Result<NamedSocketListener> {
    let name = path.to_fs_name::<GenericFilePath>()?;
    let inner = ListenerOptions::new().name(name).create_sync()?;
    Ok(NamedSocketListener { inner })
}

/// Connect to a listener bound at the given path.
pub fn connect(path: &str) -> io::Result<Stream> {
    let name = path.to_fs_name::<GenericFilePath>()?;
    Stream::connect(name)
}
