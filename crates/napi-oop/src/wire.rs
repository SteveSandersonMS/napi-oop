//! Serialization of values that cross the process boundary.
//!
//! Analogous to napi-rs's `ToNapiValue`/`FromNapiValue`, but instead of
//! converting to/from in-process `napi_value` handles, these convert to/from
//! the wire format. Phase 3 implements them for primitives (starting with the
//! `i32`s used by `add_numbers`); Phase 8 generalizes to strings, structs,
//! enums, `Buffer`, `Option`/`Result`, etc., and adds remote handles for
//! non-serializable values (the B-style fallback).

/// A value that can be serialized into the wire buffer.
pub trait ToWire {
    /// Append this value's encoding to `out`.
    fn to_wire(&self, out: &mut Vec<u8>);
}

/// A value that can be deserialized from the wire buffer.
pub trait FromWire: Sized {
    /// Decode a value from `buf`, returning it and the number of bytes consumed.
    fn from_wire(buf: &[u8]) -> Result<(Self, usize), WireError>;
}

/// Error raised while decoding a value from the wire.
#[derive(Debug)]
pub struct WireError(pub String);

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "wire decode error: {}", self.0)
    }
}

impl std::error::Error for WireError {}

// TODO(phase3): impl ToWire/FromWire for i32 (and other primitives).
