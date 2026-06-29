//! Out-of-process shims for napi-rs value types that need special wire handling:
//! `Buffer` (binary), `BigInt` (opaque u64 handle), and `External<T>` (opaque
//! JS-held handle). Source on the `napi::` path stays identical; only the wire
//! form differs from the in-proc native types.

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

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

/// Opaque BigInt mirroring napi-rs's `BigInt`. The runtime uses these purely as
/// 64-bit handle tokens, but the field layout matches napi-rs (`sign_bit` plus a
/// `words` vector) so source that constructs `BigInt { sign_bit, words: vec![h] }`
/// or calls `get_u64()` compiles unchanged. On the wire it is the single low word
/// as a u64 (the JS `bigint` MessagePack encoding).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BigInt {
    pub sign_bit: bool,
    pub words: Vec<u64>,
}

impl BigInt {
    /// Mirror napi-rs: `(sign_bit, low_word, lossless)`. `lossless` is false when
    /// the value needs more than one 64-bit word (so it wouldn't fit a u64).
    pub fn get_u64(&self) -> (bool, u64, bool) {
        let value = self.words.first().copied().unwrap_or(0);
        let lossless = self.words.len() <= 1;
        (self.sign_bit, value, lossless)
    }
}

impl From<u64> for BigInt {
    fn from(word: u64) -> Self {
        BigInt { sign_bit: false, words: vec![word] }
    }
}

impl Serialize for BigInt {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(self.words.first().copied().unwrap_or(0))
    }
}

impl<'de> Deserialize<'de> for BigInt {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        u64::deserialize(d).map(BigInt::from)
    }
}

/// A slab entry. Class instances are mutable and single-owner (`Box`, mutated in
/// place via [`with_object`]); externals are shared and immutable (`Arc`, so an
/// [`External`] handle can hand out a `&T` reference via `Deref`). Both ride the
/// same token space and GC-release path.
enum Slot {
    Class(Box<dyn std::any::Any + Send>),
    Ext(Arc<dyn std::any::Any + Send + Sync>),
}

/// Provider-side slab backing [`External`] tokens and class instances. The value
/// lives here; JS only ever holds the integer token, so it round-trips without
/// serializing the value.
static EXTERNAL_SLAB: Mutex<Option<HashMap<u64, Slot>>> = Mutex::new(None);
static EXTERNAL_NEXT: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// Per-thread count of tokens minted, sampled by the dispatcher around a call
    /// (each call runs on one worker thread) so concurrent calls don't interfere.
    static MINTED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Tokens minted on this thread so far. The dispatcher diffs this before/after a
/// call to enforce that minted `External`s surface top-level, never nested.
pub fn external_mint_count() -> u64 {
    MINTED.with(|c| c.get())
}

/// JS-held opaque handle to a Rust value mirroring napi-rs's `External<T>`. The
/// value stays provider-side in a slab and is shared via `Arc`; only a u64 token
/// crosses the boundary. Like napi-rs, `External<T>` derefs to `&T`, so source
/// that calls inner methods/fields through the handle compiles unchanged.
pub struct External<T: Send + Sync + 'static> {
    token: u64,
    value: Arc<T>,
}

impl<T: Send + Sync + 'static> External<T> {
    pub fn new(value: T) -> Self {
        let token = EXTERNAL_NEXT.fetch_add(1, Ordering::Relaxed);
        MINTED.with(|c| c.set(c.get() + 1));
        let value = Arc::new(value);
        let mut guard = EXTERNAL_SLAB.lock().unwrap();
        guard.get_or_insert_with(HashMap::new).insert(token, Slot::Ext(value.clone()));
        External { token, value }
    }

    /// Run `f` against the held value. Always live (the `Arc` is held inline).
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> Option<R> {
        Some(f(&self.value))
    }

    /// Drop the held value, releasing the slab entry.
    pub fn release(token: u64) {
        if let Some(map) = EXTERNAL_SLAB.lock().unwrap().as_mut() {
            map.remove(&token);
        }
    }
}

impl<T: Send + Sync + 'static> Deref for External<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}

impl<T: Clone + Send + Sync + 'static> External<T> {
    /// Clone out the held value, mirroring how callers copy the inner data.
    pub fn cloned(&self) -> Option<T> {
        Some((*self.value).clone())
    }
}

/// Key marking a value as an external handle on the wire: `{ "__napi_ext": <id> }`.
pub const EXTERNAL_KEY: &str = "__napi_ext";

/// Drop the value behind `token`, releasing its slab entry. Called when the peer
/// reports (via `releaseExternal`) that JS has GC'd the corresponding handle.
pub fn release_external(token: u64) {
    if let Some(map) = EXTERNAL_SLAB.lock().unwrap().as_mut() {
        map.remove(&token);
    }
}

/// Number of live External entries — for tests asserting GC-driven release.
pub fn external_slab_len() -> usize {
    EXTERNAL_SLAB.lock().unwrap().as_ref().map_or(0, |m| m.len())
}

/// Register a provider-side object (a `#[napi]` class instance) in the slab,
/// minting a top-level token. Classes ride the same slab + GC-release path as
/// `External`, so a finalized JS instance frees its Rust state — no leak.
pub fn object_new(value: Box<dyn std::any::Any + Send>) -> u64 {
    let token = EXTERNAL_NEXT.fetch_add(1, Ordering::Relaxed);
    MINTED.with(|c| c.set(c.get() + 1));
    EXTERNAL_SLAB.lock().unwrap().get_or_insert_with(HashMap::new).insert(token, Slot::Class(value));
    token
}

/// Run `f` against a live object by token, downcast to `T`. `None` if missing.
pub fn with_object<T: 'static, R>(token: u64, f: impl FnOnce(&mut T) -> R) -> Option<R> {
    let mut guard = EXTERNAL_SLAB.lock().unwrap();
    let map = guard.as_mut()?;
    match map.get_mut(&token)? {
        Slot::Class(any) => any.downcast_mut::<T>().map(f),
        Slot::Ext(_) => None,
    }
}

/// Look up a live external by token and clone its `Arc<T>`. Used by the macro's
/// `&External<T>` decode path to rebuild a handle pointing at the resident value.
fn external_lookup<T: Send + Sync + 'static>(token: u64) -> Option<Arc<T>> {
    let guard = EXTERNAL_SLAB.lock().unwrap();
    match guard.as_ref()?.get(&token)? {
        Slot::Ext(arc) => arc.clone().downcast::<T>().ok(),
        Slot::Class(_) => None,
    }
}

impl<T: Send + Sync + 'static> Serialize for External<T> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut m = s.serialize_map(Some(1))?;
        m.serialize_entry(EXTERNAL_KEY, &self.token)?;
        m.end()
    }
}

impl<'de, T: Send + Sync + 'static> Deserialize<'de> for External<T> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let map: HashMap<String, u64> = HashMap::deserialize(d)?;
        let token = *map
            .get(EXTERNAL_KEY)
            .ok_or_else(|| serde::de::Error::custom("not an external handle"))?;
        let value = external_lookup::<T>(token)
            .ok_or_else(|| serde::de::Error::custom("external handle no longer live"))?;
        Ok(External { token, value })
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

    #[test]
    fn release_external_frees_the_slab_entry() {
        let e = External::new(vec![1i32, 2, 3]);
        let token = e.token;
        let wire = to_wire(&e).unwrap();
        assert_eq!(e.with(|v| v.len()), Some(3));
        release_external(token);
        // After release the token no longer resolves — no leak, no double-free.
        assert!(from_wire::<External<Vec<i32>>>(wire).is_err());
    }

    #[test]
    fn external_derefs_to_inner() {
        let e = External::new(vec![7i32, 8, 9]);
        // napi-rs External<T> derefs to &T; method calls go through the handle.
        assert_eq!(e.len(), 3);
        assert_eq!(e[1], 8);
    }
}
