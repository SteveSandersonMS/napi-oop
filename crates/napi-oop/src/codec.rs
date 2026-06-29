//! RPC framing and message codec.
//!
//! The wire is a sequence of length-prefixed frames:
//!
//! ```text
//! frame = u32_be(len) ++ payload
//! ```
//!
//! `payload` is a [`Message`] encoded with MessagePack (`rmp-serde`, struct
//! fields as named maps so the Node side decodes ergonomic objects). The
//! message set is full-duplex — either peer may originate a [`Message::Request`]
//! (needed for callbacks).
//!
//! MessagePack is used (rather than a Rust-only format like postcard/bincode)
//! because the same frames are decoded by the Node runtime, and MessagePack has
//! mature libraries on both sides. The format is isolated behind
//! [`write_message`]/[`read_message`] so it can be swapped later.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

/// Correlation id used to match a [`Response`]/[`ErrorMsg`] to its [`Request`].
pub type CorrelationId = u64;

/// Identifier for a remote handle (e.g. a JS callback proxied to Rust). Used by
/// the B-style fallback added in a later phase.
pub type HandleId = u64;

/// Which side a peer plays in the example boundary. (Informational in the
/// handshake; the transport itself is symmetric.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Hosts the `#[napi]` functions (the Rust side).
    Provider,
    /// Invokes functions on the provider (the Node side).
    Caller,
}

/// The handshake message: announces protocol version, role, and the function
/// registry the sender exposes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    pub version: u32,
    pub role: Role,
    pub functions: Vec<String>,
}

/// A call from one peer to a function exposed by the other.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: CorrelationId,
    #[serde(rename = "fn")]
    pub function: String,
    /// Arguments, as dynamic MessagePack values for now. Phase 3 replaces these
    /// with statically-typed `ToWire`/`FromWire` codecs.
    pub args: Vec<rmpv::Value>,
}

/// A successful reply to a [`Request`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub id: CorrelationId,
    pub result: rmpv::Value,
}

/// A failed reply to a [`Request`] (a thrown JS exception or a Rust `Err`/panic).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorMsg {
    pub id: CorrelationId,
    pub message: String,
}

/// Invoke a remote callback handle (reverse direction). Fire-and-forget, like
/// napi's `ThreadsafeFunction`: there is no reply, so no correlation id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallbackInvoke {
    pub handle: HandleId,
    pub args: Vec<rmpv::Value>,
}

/// Release a remote handle so the owning side can drop it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Release {
    pub handle: HandleId,
}

/// Release an `External` token so the provider can drop its slab entry. Sent by
/// the caller when JS has GC'd the corresponding handle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReleaseExternal {
    pub token: u64,
}

/// The full set of messages carried over the wire. Internally tagged on a
/// `type` field, so encoded frames look like `{ type: "request", id, fn, args }`
/// — directly mirrored by the Node runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Message {
    Hello(Hello),
    Request(Request),
    Response(Response),
    Error(ErrorMsg),
    CallbackInvoke(CallbackInvoke),
    Release(Release),
    ReleaseExternal(ReleaseExternal),
}

/// Write a single length-prefixed MessagePack frame and flush it.
pub fn write_message<W: Write>(w: &mut W, message: &Message) -> io::Result<()> {
    let bytes = rmp_serde::to_vec_named(message)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let len = u32::try_from(bytes.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "frame exceeds u32 length"))?;
    w.write_all(&len.to_be_bytes())?;
    w.write_all(&bytes)?;
    w.flush()
}

/// Read a single length-prefixed MessagePack frame. Returns `Ok(None)` on a
/// clean EOF at a frame boundary.
pub fn read_message<R: Read>(r: &mut R) -> io::Result<Option<Message>> {
    let mut len_buf = [0u8; 4];
    if let Err(e) = r.read_exact(&mut len_buf) {
        if e.kind() == io::ErrorKind::UnexpectedEof {
            return Ok(None);
        }
        return Err(e);
    }
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    let message =
        rmp_serde::from_slice(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(Some(message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn round_trip(message: &Message) -> Message {
        let mut buf = Vec::new();
        write_message(&mut buf, message).unwrap();
        let mut cursor = Cursor::new(buf);
        read_message(&mut cursor).unwrap().expect("a message")
    }

    #[test]
    fn hello_round_trips() {
        let msg = Message::Hello(Hello {
            version: crate::PROTOCOL_VERSION,
            role: Role::Provider,
            functions: vec!["add_numbers".into()],
        });
        assert_eq!(round_trip(&msg), msg);
    }

    #[test]
    fn request_and_response_round_trip() {
        let req = Message::Request(Request {
            id: 7,
            function: "add_numbers".into(),
            args: vec![rmpv::Value::from(2i64), rmpv::Value::from(3i64)],
        });
        assert_eq!(round_trip(&req), req);

        let resp = Message::Response(Response {
            id: 7,
            result: rmpv::Value::from(5i64),
        });
        assert_eq!(round_trip(&resp), resp);
    }

    #[test]
    fn error_round_trips() {
        let msg = Message::Error(ErrorMsg {
            id: 9,
            message: "boom".into(),
        });
        assert_eq!(round_trip(&msg), msg);
    }

    #[test]
    fn clean_eof_returns_none() {
        let mut empty = Cursor::new(Vec::new());
        assert_eq!(read_message(&mut empty).unwrap(), None);
    }

    #[test]
    fn back_to_back_frames_decode_in_order() {
        let a = Message::Release(Release { handle: 1 });
        let b = Message::Release(Release { handle: 2 });
        let mut buf = Vec::new();
        write_message(&mut buf, &a).unwrap();
        write_message(&mut buf, &b).unwrap();
        let mut cursor = Cursor::new(buf);
        assert_eq!(read_message(&mut cursor).unwrap().unwrap(), a);
        assert_eq!(read_message(&mut cursor).unwrap().unwrap(), b);
        assert_eq!(read_message(&mut cursor).unwrap(), None);
    }
}
