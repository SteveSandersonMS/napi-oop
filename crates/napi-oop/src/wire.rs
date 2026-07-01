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
///
/// Structs are encoded as **named maps** (not positional arrays): a
/// `#[napi(object)]` value must reach JS as `{ field: … }` with named,
/// camelCased keys, exactly like napi-rs. rmpv's own `to_value` serializes
/// structs as arrays (dropping field names), so we serialize through rmp-serde
/// in `with_struct_map` mode and read the bytes back into an [`rmpv::Value`].
pub fn to_wire<T: Serialize + ?Sized>(value: &T) -> Result<Value, WireError> {
    let mut buf = Vec::new();
    let mut ser = rmp_serde::Serializer::new(&mut buf).with_struct_map();
    value
        .serialize(&mut ser)
        .map_err(|e| WireError(e.to_string()))?;
    rmpv::decode::read_value(&mut &buf[..]).map_err(|e| WireError(e.to_string()))
}

/// Decode a value from the dynamic wire representation.
pub fn from_wire<T: DeserializeOwned>(value: Value) -> Result<T, WireError> {
    rmpv::ext::from_value(normalize_integral_floats(value)).map_err(|e| WireError(e.to_string()))
}

/// Recursively rewrite integral floating-point values into integers.
///
/// JavaScript has a single `number` type, so an integer like `Date.now()` is
/// still a `number`. MessagePack encoders on the JS side (msgpackr) encode any
/// integer wider than 32 bits as a float64 rather than an int64, so a value
/// destined for a Rust `i64`/`u64` parameter arrives as [`Value::F64`] and
/// `rmpv` refuses to decode it ("invalid type: floating point, expected i64").
///
/// Because JS cannot distinguish `2` from `2.0`, and every integral float below
/// 2^53 round-trips losslessly, we canonicalize integral floats to integers
/// before deserializing. This lets integer parameters decode correctly while
/// float parameters are unaffected: `rmpv` already coerces an integer back to
/// `f64` (a Rust `f64` param routinely receives an integral JS number encoded as
/// a MessagePack int, e.g. `1.0`), so the reverse mapping is already required and
/// exercised.
fn normalize_integral_floats(value: Value) -> Value {
    match value {
        Value::F64(f) if f.fract() == 0.0 => integral_float_to_int(f).unwrap_or(Value::F64(f)),
        Value::F32(f) if f.fract() == 0.0 => {
            integral_float_to_int(f as f64).unwrap_or(Value::F32(f))
        }
        Value::Array(items) => {
            Value::Array(items.into_iter().map(normalize_integral_floats).collect())
        }
        Value::Map(entries) => Value::Map(
            entries
                .into_iter()
                .map(|(k, v)| (normalize_integral_floats(k), normalize_integral_floats(v)))
                .collect(),
        ),
        other => other,
    }
}

/// Convert an integral `f64` to a MessagePack integer if it fits exactly in
/// `i64` or `u64`; otherwise return `None` to leave it as a float.
fn integral_float_to_int(f: f64) -> Option<Value> {
    if f >= i64::MIN as f64 && f <= i64::MAX as f64 {
        Some(Value::from(f as i64))
    } else if f >= 0.0 && f <= u64::MAX as f64 {
        Some(Value::from(f as u64))
    } else {
        None
    }
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
    Value::Map(vec![(
        Value::from(crate::types::EXTERNAL_KEY),
        Value::from(token),
    )])
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

    #[test]
    fn large_integer_encoded_as_float_decodes_to_i64() {
        // msgpackr encodes integers wider than 32 bits (e.g. `Date.now()`) as a
        // MessagePack float64, not an int64. Such a value must still decode into
        // an integer parameter rather than failing with "expected i64".
        let timestamp = 1_782_910_509_260i64;
        let v = Value::F64(timestamp as f64);
        assert_eq!(from_wire::<i64>(v).unwrap(), timestamp);

        // A value above i64::MAX but within JS's exact-integer range routes
        // through the u64 branch. (JS numbers are only exact below 2^53, so this
        // is the practical ceiling for a losslessly-transported integer.)
        let big_unsigned = 1u64 << 52;
        let v = Value::F64(big_unsigned as f64);
        assert_eq!(from_wire::<u64>(v).unwrap(), big_unsigned);
    }

    #[test]
    fn integral_float_still_decodes_to_f64() {
        // A float parameter that receives an integral value must be unaffected by
        // the integer normalization.
        let v = Value::F64(3.0);
        assert_eq!(from_wire::<f64>(v).unwrap(), 3.0);
    }

    #[test]
    fn fractional_float_is_preserved() {
        let v = Value::F64(3.5);
        assert_eq!(from_wire::<f64>(v).unwrap(), 3.5);
    }

    #[test]
    fn integral_floats_nested_in_struct_decode() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Event {
            timestamp_ms: i64,
            ratio: f64,
        }
        // Simulate the msgpackr wire form: the large integer arrived as a float.
        let wire = Value::Map(vec![
            (
                Value::from("timestamp_ms"),
                Value::F64(1_782_910_509_260f64),
            ),
            (Value::from("ratio"), Value::F64(0.5)),
        ]);
        let decoded = from_wire::<Event>(wire).unwrap();
        assert_eq!(
            decoded,
            Event {
                timestamp_ms: 1_782_910_509_260,
                ratio: 0.5
            }
        );
    }
}
