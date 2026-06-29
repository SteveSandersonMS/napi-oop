//! Out-of-process shims for napi-rs value types that need special wire handling:
//! `Buffer` (binary), `BigInt` (opaque u64 handle), and `External<T>` (opaque
//! JS-held handle). Source on the `napi::` path stays identical; only the wire
//! form differs from the in-proc native types.

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Binary blob mirroring napi-rs's `Buffer`. On the wire it is MessagePack `bin`
/// (serde bytes), not a number array, so large payloads stay compact.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Buffer(Vec<u8>);

impl Buffer {
    pub fn to_vec(&self) -> Vec<u8> {
        self.0.clone()
    }
}

impl From<Vec<u8>> for Buffer {
    fn from(v: Vec<u8>) -> Self {
        Buffer(v)
    }
}

impl From<Buffer> for Vec<u8> {
    fn from(b: Buffer) -> Self {
        b.0
    }
}

impl AsRef<[u8]> for Buffer {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl Deref for Buffer {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl Serialize for Buffer {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(&self.0)
    }
}

impl<'de> Deserialize<'de> for Buffer {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        serde_bytes::deserialize(d).map(Buffer)
    }
}

/// Opaque 64-bit handle mirroring napi-rs's `BigInt`. Every BigInt that crosses
/// this boundary is a handle token (fits u64), so the wire form is a single u64
/// (matching the JS `bigint` MessagePack encoding), not a struct.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BigInt {
    pub sign_bit: bool,
    pub words: u64,
}

impl BigInt {
    pub fn get_u64(&self) -> (bool, u64, bool) {
        (self.sign_bit, self.words, false)
    }
}

impl From<u64> for BigInt {
    fn from(words: u64) -> Self {
        BigInt { sign_bit: false, words }
    }
}

impl Serialize for BigInt {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(self.words)
    }
}

impl<'de> Deserialize<'de> for BigInt {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        u64::deserialize(d).map(BigInt::from)
    }
}

/// Provider-side slab backing [`External`] tokens. The value lives here; JS only
/// ever holds the integer token, so it round-trips without serializing the value.
static EXTERNAL_SLAB: Mutex<Option<HashMap<u64, Box<dyn std::any::Any + Send>>>> = Mutex::new(None);
static EXTERNAL_NEXT: AtomicU64 = AtomicU64::new(1);

/// JS-held opaque handle to a Rust value mirroring napi-rs's `External<T>`. The
/// value stays provider-side in a slab; only a u64 token crosses the boundary.
pub struct External<T: Send + 'static> {
    token: u64,
    _marker: std::marker::PhantomData<T>,
}

impl<T: Send + 'static> External<T> {
    pub fn new(value: T) -> Self {
        let token = EXTERNAL_NEXT.fetch_add(1, Ordering::Relaxed);
        let mut guard = EXTERNAL_SLAB.lock().unwrap();
        guard.get_or_insert_with(HashMap::new).insert(token, Box::new(value));
        External { token, _marker: std::marker::PhantomData }
    }

    /// Run `f` against the held value, if the token is still live.
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> Option<R> {
        let guard = EXTERNAL_SLAB.lock().unwrap();
        let map = guard.as_ref()?;
        let any = map.get(&self.token)?;
        any.downcast_ref::<T>().map(f)
    }

    /// Drop the held value, releasing the slab entry.
    pub fn release(token: u64) {
        if let Some(map) = EXTERNAL_SLAB.lock().unwrap().as_mut() {
            map.remove(&token);
        }
    }
}

impl<T: Clone + Send + 'static> External<T> {
    /// Clone out the held value, mirroring how callers copy the inner data.
    pub fn cloned(&self) -> Option<T> {
        self.with(|v| v.clone())
    }
}

/// Key marking a value as an external handle on the wire: `{ "__napi_ext": <id> }`.
pub const EXTERNAL_KEY: &str = "__napi_ext";

impl<T: Send + 'static> Serialize for External<T> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut m = s.serialize_map(Some(1))?;
        m.serialize_entry(EXTERNAL_KEY, &self.token)?;
        m.end()
    }
}

impl<'de, T: Send + 'static> Deserialize<'de> for External<T> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let map: HashMap<String, u64> = HashMap::deserialize(d)?;
        let token = *map
            .get(EXTERNAL_KEY)
            .ok_or_else(|| serde::de::Error::custom("not an external handle"))?;
        Ok(External { token, _marker: std::marker::PhantomData })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{from_wire, to_wire};

    #[test]
    fn buffer_round_trips_as_bytes() {
        let b = Buffer::from(vec![1u8, 2, 3, 255]);
        let v = to_wire(&b).unwrap();
        assert!(matches!(v, rmpv::Value::Binary(_)));
        assert_eq!(from_wire::<Buffer>(v).unwrap(), b);
    }

    #[test]
    fn bigint_round_trips_as_u64() {
        let big = BigInt::from(9_000_000_000_000_000_000u64);
        let v = to_wire(&big).unwrap();
        assert_eq!(from_wire::<BigInt>(v).unwrap().words, big.words);
    }

    #[test]
    fn external_round_trips_via_token() {
        let e = External::new(vec![10i32, 20, 30]);
        let v = to_wire(&e).unwrap();
        let back: External<Vec<i32>> = from_wire(v).unwrap();
        assert_eq!(back.cloned(), Some(vec![10, 20, 30]));
    }
}
