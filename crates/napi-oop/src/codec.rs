//! RPC framing and message codec.
//!
//! Phase 2 replaces the spike's length-prefixed JSON with a length-prefixed
//! **binary** encoding (serde + a compact format such as `postcard`), wrapping a
//! small versioned message envelope:
//!
//! ```text
//! frame = u32_be(len) ++ payload
//! payload = { version, msg_type, correlation_id, target_id, body }
//! msg_type ∈ { Hello, Request, Response, Error,
//!              CallbackInvoke, CallbackResult, Release }
//! ```
//!
//! Designed full-duplex: requests may originate from either peer (needed for
//! callbacks).

// TODO(phase2): frame reader/writer + message envelope (de)serialization.
