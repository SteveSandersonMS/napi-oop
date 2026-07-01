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
#[cfg(not(any(target_os = "linux", target_os = "android")))]
use interprocess::local_socket::GenericFilePath;
#[cfg(any(target_os = "linux", target_os = "android"))]
use interprocess::local_socket::GenericNamespaced;
use interprocess::local_socket::{
    Listener, ListenerNonblockingMode, ListenerOptions, Name, Stream,
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

/// Build a platform-appropriate local-socket [`Name`] from the wire string.
///
/// On Linux the string is a bare name bound in the *abstract namespace*
/// (leading-NUL address, no filesystem entry); everywhere else it is a
/// filesystem socket path (macOS/BSD) or named-pipe path (Windows). Both peers
/// run on the same OS, so the parent and child always agree on the encoding.
fn to_name(name: &str) -> io::Result<Name<'_>> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        name.to_ns_name::<GenericNamespaced>()
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        name.to_fs_name::<GenericFilePath>()
    }
}

/// Bind a listener for the given socket name (abstract name on Linux, filesystem
/// socket path on macOS/BSD, named-pipe path on Windows).
pub fn listen(path: &str) -> io::Result<NamedSocketListener> {
    let name = to_name(path)?;
    let inner = ListenerOptions::new().name(name).create_sync()?;
    Ok(NamedSocketListener { inner })
}

/// Connect to a listener bound at the given socket name.
pub fn connect(path: &str) -> io::Result<Stream> {
    let name = to_name(path)?;
    Stream::connect(name)
}
