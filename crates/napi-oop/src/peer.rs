//! The peer: connection bootstrap, handshake, and (later) the message router.
//!
//! This phase implements the **handshake**. The bootstrap (parent generates a
//! socket path, spawns the child, child connects) and the correlation-id
//! **router** with re-entrant callback handling arrive in later phases.

use std::io::{self, Read, Write};

use crate::codec::{read_message, write_message, Hello, Message};
use crate::PROTOCOL_VERSION;

/// Perform the symmetric handshake over an already-connected stream: send our
/// [`Hello`], read the peer's, and verify the protocol versions match.
///
/// Both peers send before reading; the stream is full-duplex so this does not
/// deadlock. Returns the peer's [`Hello`] (its role + advertised functions).
pub fn handshake<S: Read + Write>(stream: &mut S, local: Hello) -> io::Result<Hello> {
    write_message(stream, &Message::Hello(local))?;
    match read_message(stream)? {
        Some(Message::Hello(peer)) => {
            if peer.version != PROTOCOL_VERSION {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "protocol version mismatch: local {PROTOCOL_VERSION}, peer {}",
                        peer.version
                    ),
                ));
            }
            Ok(peer)
        }
        Some(other) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected Hello during handshake, got {other:?}"),
        )),
        None => Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "peer closed during handshake",
        )),
    }
}
