//! Serialization of values that cross the process boundary.
//!
//! Rather than hand-writing a codec for every type, we lean on **serde**: any
//! type that is `Serialize`/`DeserializeOwned` can cross the boundary for free
//! (including everything you get from `#[derive(Serialize, Deserialize)]` —
//! structs, enums, `Vec`, `Option`, maps, …). The on-wire form is MessagePack
//! (see [`crate::codec`]); here we bridge a value to/from the dynamic
//! [`rmpv::Value`] carried in a [`crate::codec::Request`]/[`crate::codec::Response`].
//!
//! [`ToWire`]/[`FromWire`] are therefore just blanket *aliases* over serde — they
//! document the boundary and give a single place to hang future bounds. The only
//! values serde alone can't carry are **live references** (callbacks, remote
//! handles): those are handled by giving the relevant napi types custom serde
//! impls that encode a handle id (added in the callbacks/handles phase), so the
//! user's `#[napi]` source still never changes.

use rmpv::Value;
use serde::{de::DeserializeOwned, Serialize};

/// Any value that can be serialized onto the wire. Blanket-implemented for every
/// [`serde::Serialize`] type — no per-type impl required.
pub trait ToWire: Serialize {}
impl<T: Serialize + ?Sized> ToWire for T {}

/// Any value that can be deserialized from the wire. Blanket-implemented for
/// every [`serde::de::DeserializeOwned`] type.
pub trait FromWire: DeserializeOwned {}
impl<T: DeserializeOwned> FromWire for T {}

/// Error raised while converting a value to/from the wire representation.
#[derive(Debug)]
pub struct WireError(pub String);

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "wire conversion error: {}", self.0)
    }
}

impl std::error::Error for WireError {}

/// Encode a value into the dynamic wire representation.
pub fn to_wire<T: Serialize + ?Sized>(value: &T) -> Result<Value, WireError> {
    rmpv::ext::to_value(value).map_err(|e| WireError(e.to_string()))
}

/// Decode a value from the dynamic wire representation.
pub fn from_wire<T: DeserializeOwned>(value: Value) -> Result<T, WireError> {
    rmpv::ext::from_value(value).map_err(|e| WireError(e.to_string()))
}

/// Key marking a value as a remote callback handle: `{ "__napi_cb": <id> }`.
/// JS function args become this on the wire; the macro turns it into a Rust
/// closure that invokes the callback back across the boundary.
pub const CALLBACK_KEY: &str = "__napi_cb";

/// Extract a callback handle id from its wire marker, or error if not one.
pub fn callback_handle(value: &Value) -> Result<u64, WireError> {
    if let Value::Map(entries) = value {
        for (k, v) in entries {
            if k.as_str() == Some(CALLBACK_KEY) {
                if let Some(id) = v.as_u64() {
                    return Ok(id);
                }
            }
        }
    }
    Err(WireError("argument is not a callback handle".into()))
}

/// Build the wire marker for an external/object handle: `{ "__napi_ext": <id> }`.
pub fn external_marker(token: u64) -> Value {
    Value::Map(vec![(Value::from(crate::types::EXTERNAL_KEY), Value::from(token))])
}

/// Extract an external/object handle token from its wire marker.
pub fn external_handle(value: &Value) -> Result<u64, String> {
    if let Value::Map(entries) = value {
        for (k, v) in entries {
            if k.as_str() == Some(crate::types::EXTERNAL_KEY) {
                if let Some(id) = v.as_u64() {
                    return Ok(id);
                }
            }
        }
    }
    Err("argument is not an object/external handle".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[test]
    fn primitives_round_trip() {
        let v = to_wire(&5i32).unwrap();
        assert_eq!(from_wire::<i32>(v).unwrap(), 5);

        let v = to_wire("hello").unwrap();
        assert_eq!(from_wire::<String>(v).unwrap(), "hello");
    }

    #[test]
    fn derived_struct_round_trips_for_free() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Point {
            x: i32,
            y: i32,
        }
        let p = Point { x: 1, y: -2 };
        let v = to_wire(&p).unwrap();
        assert_eq!(from_wire::<Point>(v).unwrap(), p);
    }

    #[test]
    fn collections_round_trip() {
        let xs = vec![Some(1u8), None, Some(3)];
        let v = to_wire(&xs).unwrap();
        assert_eq!(from_wire::<Vec<Option<u8>>>(v).unwrap(), xs);
    }
}
