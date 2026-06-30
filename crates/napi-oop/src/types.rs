//! Out-of-process shims for napi-rs value types that need special wire handling:
//! `Buffer` (binary), `BigInt` (opaque u64 handle), and `External<T>` (opaque
//! JS-held handle). Source on the `napi::` path stays identical; only the wire
//! form differs from the in-proc native types.

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use napi::bindgen_prelude::{self as napi_bp, FromNapiValue, ToNapiValue, TypeName, ValidateNapiValue};
use napi::sys;
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

// In-proc napi bridge: delegate to the real napi `Buffer` so this unified type can
// be a `#[napi]` parameter/return when the cdylib is loaded directly by Node.
impl TypeName for Buffer {
    fn type_name() -> &'static str {
        napi_bp::Buffer::type_name()
    }
    fn value_type() -> napi::ValueType {
        napi_bp::Buffer::value_type()
    }
}

impl ValidateNapiValue for Buffer {
    unsafe fn validate(env: sys::napi_env, napi_val: sys::napi_value) -> napi::Result<sys::napi_value> {
        unsafe { napi_bp::Buffer::validate(env, napi_val) }
    }
}

impl FromNapiValue for Buffer {
    unsafe fn from_napi_value(env: sys::napi_env, napi_val: sys::napi_value) -> napi::Result<Self> {
        let real = unsafe { napi_bp::Buffer::from_napi_value(env, napi_val)? };
        Ok(Buffer(real.to_vec()))
    }
}

impl ToNapiValue for Buffer {
    unsafe fn to_napi_value(env: sys::napi_env, val: Self) -> napi::Result<sys::napi_value> {
        unsafe { napi_bp::Buffer::to_napi_value(env, napi_bp::Buffer::from(val.0)) }
    }
}

/// BigInt mirroring napi-rs's `BigInt`, with the same field layout (`sign_bit`
/// plus a little-endian `words` vector) so source that constructs
/// `BigInt { sign_bit, words }` or calls `get_u64()` compiles unchanged. On the
/// wire it rides as msgpackr's `0x42` bigint extension (big-endian two's
/// complement), so values of any width round-trip to a JS `bigint` losslessly —
/// matching the in-proc napi door, which carries the native full-precision value.
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
        BigInt {
            sign_bit: false,
            words: vec![word],
        }
    }
}

/// MessagePack extension type id for arbitrary-precision integers, matching
/// msgpackr's `useBigIntExtension` encoding. The JS runtime decodes ext-`0x42`
/// unconditionally to a `bigint`, and emits it for values that overflow 64 bits.
const BIGINT_EXT_TYPE: i8 = 0x42;

/// Encode a sign-magnitude [`BigInt`] (napi's `{ sign_bit, words }`, words little
/// -endian) as the big-endian two's-complement byte string msgpackr uses for its
/// `0x42` bigint extension. Used so a `BigInt` of any width round-trips losslessly
/// to a JS `bigint` over the wire — matching the in-proc napi door, where the
/// native `BigInt` is already full precision.
fn bigint_to_twos_complement_be(sign_bit: bool, words: &[u64]) -> Vec<u8> {
    // Magnitude, big-endian (high word first), with leading zero bytes stripped.
    let mut be = Vec::with_capacity(words.len() * 8);
    for &w in words.iter().rev() {
        be.extend_from_slice(&w.to_be_bytes());
    }
    let first_nonzero = be.iter().position(|&b| b != 0).unwrap_or(be.len());
    let mut mag = be[first_nonzero..].to_vec();
    if mag.is_empty() {
        return vec![0]; // value is zero
    }
    if !sign_bit {
        // Positive: keep the sign bit clear so msgpackr reads it as non-negative.
        if mag[0] & 0x80 != 0 {
            mag.insert(0, 0x00);
        }
        mag
    } else {
        // Negative: emit two's complement, widening by a byte if needed so the
        // result's high bit is set (i.e. reads back as negative).
        if mag[0] & 0x80 != 0 {
            mag.insert(0, 0x00);
        }
        let mut carry = 1u16;
        for b in mag.iter_mut().rev() {
            let v = (!*b as u16) + carry;
            *b = (v & 0xff) as u8;
            carry = v >> 8;
        }
        mag
    }
}

/// Decode a big-endian two's-complement byte string (msgpackr's `0x42` bigint
/// extension) back into napi's sign-magnitude `{ sign_bit, words }` form.
fn bigint_from_twos_complement_be(bytes: &[u8]) -> (bool, Vec<u64>) {
    if bytes.is_empty() {
        return (false, vec![0]);
    }
    let negative = bytes[0] & 0x80 != 0;
    // Recover the unsigned magnitude (undo two's complement for negatives).
    let mag: Vec<u8> = if negative {
        let mut inv: Vec<u8> = bytes.iter().map(|b| !b).collect();
        let mut carry = 1u16;
        for b in inv.iter_mut().rev() {
            let v = (*b as u16) + carry;
            *b = (v & 0xff) as u8;
            carry = v >> 8;
        }
        inv
    } else {
        bytes.to_vec()
    };
    // Left-pad to a multiple of 8 bytes, then fold into little-endian u64 words.
    let pad = (8 - mag.len() % 8) % 8;
    let mut padded = vec![0u8; pad];
    padded.extend_from_slice(&mag);
    let mut words = Vec::with_capacity(padded.len() / 8);
    let mut i = padded.len();
    while i >= 8 {
        let chunk: [u8; 8] = padded[i - 8..i].try_into().unwrap();
        words.push(u64::from_be_bytes(chunk));
        i -= 8;
    }
    while words.len() > 1 && *words.last().unwrap() == 0 {
        words.pop();
    }
    (negative, words)
}

impl Serialize for BigInt {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        // Always ride the wire as a msgpack ext (`0x42`): rmpv would compact a
        // plain integer to its smallest form, which msgpackr then decodes as a JS
        // `number`, not a `bigint`. The ext is decoded unconditionally as a
        // `bigint`, preserving the full value and the JS type.
        let bytes = bigint_to_twos_complement_be(self.sign_bit, &self.words);
        s.serialize_newtype_struct(
            rmp_serde::MSGPACK_EXT_STRUCT_NAME,
            &(BIGINT_EXT_TYPE, serde_bytes::Bytes::new(&bytes)),
        )
    }
}

impl<'de> Deserialize<'de> for BigInt {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct BigIntVisitor;

        impl<'de> serde::de::Visitor<'de> for BigIntVisitor {
            type Value = BigInt;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an integer or a msgpack bigint extension")
            }

            // A JS `bigint` that fits 64 bits arrives as a native msgpack int.
            fn visit_u64<E>(self, v: u64) -> Result<BigInt, E> {
                Ok(BigInt::from(v))
            }

            fn visit_i64<E>(self, v: i64) -> Result<BigInt, E> {
                Ok(BigInt {
                    sign_bit: v < 0,
                    words: vec![v.unsigned_abs()],
                })
            }

            // A wider `bigint` arrives as the `0x42` ext: a `(tag, bytes)` pair.
            fn visit_newtype_struct<D>(self, de: D) -> Result<BigInt, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let (_tag, bytes): (i8, serde_bytes::ByteBuf) = Deserialize::deserialize(de)?;
                let (sign_bit, words) = bigint_from_twos_complement_be(&bytes);
                Ok(BigInt { sign_bit, words })
            }
        }

        d.deserialize_any(BigIntVisitor)
    }
}

// In-proc napi bridge: delegate to the real napi `BigInt` (identical field shape).
impl TypeName for BigInt {
    fn type_name() -> &'static str {
        napi_bp::BigInt::type_name()
    }
    fn value_type() -> napi::ValueType {
        napi_bp::BigInt::value_type()
    }
}

impl ValidateNapiValue for BigInt {}

impl FromNapiValue for BigInt {
    unsafe fn from_napi_value(env: sys::napi_env, napi_val: sys::napi_value) -> napi::Result<Self> {
        let real = unsafe { napi_bp::BigInt::from_napi_value(env, napi_val)? };
        Ok(BigInt {
            sign_bit: real.sign_bit,
            words: real.words,
        })
    }
}

impl ToNapiValue for BigInt {
    unsafe fn to_napi_value(env: sys::napi_env, val: Self) -> napi::Result<sys::napi_value> {
        unsafe {
            napi_bp::BigInt::to_napi_value(
                env,
                napi_bp::BigInt {
                    sign_bit: val.sign_bit,
                    words: val.words,
                },
            )
        }
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
/// value is shared via `Arc`. On the **out-of-proc** path it stays provider-side
/// in a slab and only a u64 token crosses the boundary; the token is minted
/// lazily the first time the handle is serialized (so the in-proc napi path,
/// which never serializes, never touches the slab and cannot leak it). On the
/// **in-proc** path the `Arc<T>` rides inside a real napi `External<Arc<T>>`.
/// Like napi-rs, `External<T>` derefs to `&T`, so source that calls inner
/// methods/fields through the handle compiles unchanged.
pub struct External<T: Send + Sync + 'static> {
    /// Wire token: `0` until minted on first serialize (out-of-proc only).
    token: AtomicU64,
    value: Arc<T>,
}

impl<T: Send + Sync + 'static> External<T> {
    pub fn new(value: T) -> Self {
        External {
            token: AtomicU64::new(0),
            value: Arc::new(value),
        }
    }

    /// Mint (or return the existing) wire token, inserting the value into the
    /// provider slab on first call. Only invoked on the out-of-proc serialize path.
    fn wire_token(&self) -> u64 {
        let existing = self.token.load(Ordering::Relaxed);
        if existing != 0 {
            return existing;
        }
        let token = EXTERNAL_NEXT.fetch_add(1, Ordering::Relaxed);
        MINTED.with(|c| c.set(c.get() + 1));
        EXTERNAL_SLAB
            .lock()
            .unwrap()
            .get_or_insert_with(HashMap::new)
            .insert(token, Slot::Ext(self.value.clone()));
        self.token.store(token, Ordering::Relaxed);
        token
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
    EXTERNAL_SLAB
        .lock()
        .unwrap()
        .as_ref()
        .map_or(0, |m| m.len())
}

/// Register a provider-side object (a `#[napi]` class instance) in the slab,
/// minting a top-level token. Classes ride the same slab + GC-release path as
/// `External`, so a finalized JS instance frees its Rust state — no leak.
pub fn object_new(value: Box<dyn std::any::Any + Send>) -> u64 {
    let token = EXTERNAL_NEXT.fetch_add(1, Ordering::Relaxed);
    MINTED.with(|c| c.set(c.get() + 1));
    EXTERNAL_SLAB
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(token, Slot::Class(value));
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

/// Marker trait the macro implements for every `#[napi]` class. It lets the
/// return-encoder (below) recognise a class instance by type — without the macro
/// having to know the full set of class names — so any fn returning a class (its
/// own, *another* class, or a free-fn factory) mints the instance into the slab
/// as an external handle rather than trying to serialize it. `Send + 'static`
/// matches the slab's `Box<dyn Any + Send>` requirement.
pub trait NapiClass: Send + 'static {}

/// Owned return value awaiting wire encoding. Generated dispatch thunks wrap the
/// (already Result-unwrapped) return in this and call `.napi_oop_encode()`; the
/// two impls below dispatch on whether the type is a class.
#[doc(hidden)]
pub struct ReturnValue<T>(std::cell::RefCell<Option<T>>);

impl<T> ReturnValue<T> {
    pub fn new(value: T) -> Self {
        ReturnValue(std::cell::RefCell::new(Some(value)))
    }
}

/// Class returns: mint the owned instance into the slab and surface its token as
/// an external handle. Implemented for `ReturnValue<T>` directly so this method
/// wins (by autoref specialization) over the `Serialize` fallback below whenever
/// `T: NapiClass`.
#[doc(hidden)]
pub trait EncodeClassReturn {
    fn napi_oop_encode(&self) -> Result<rmpv::Value, String>;
}

impl<T: NapiClass> EncodeClassReturn for ReturnValue<T> {
    fn napi_oop_encode(&self) -> Result<rmpv::Value, String> {
        let value = self
            .0
            .borrow_mut()
            .take()
            .expect("ReturnValue encoded once");
        Ok(crate::wire::external_marker(object_new(Box::new(value))))
    }
}

/// Non-class returns: serialize by value onto the wire. Implemented for
/// `&ReturnValue<T>`, one autoref further out than the class impl, so it is the
/// lower-priority fallback chosen only when `T` is not a class.
#[doc(hidden)]
pub trait EncodeSerializeReturn {
    fn napi_oop_encode(&self) -> Result<rmpv::Value, String>;
}

impl<T: Serialize> EncodeSerializeReturn for &ReturnValue<T> {
    fn napi_oop_encode(&self) -> Result<rmpv::Value, String> {
        let borrow = self.0.borrow();
        let value = borrow.as_ref().expect("ReturnValue encoded once");
        crate::wire::to_wire(value).map_err(|e| e.to_string())
    }
}

/// Encode an owned (already Result-unwrapped) provider return for the wire,
/// dispatching at compile time: class instances (`T: NapiClass`) mint into the
/// slab as external handles; everything else serializes by value. The autoref on
/// the receiver selects the class impl when applicable and the serialize impl
/// otherwise.
#[macro_export]
#[doc(hidden)]
macro_rules! __napi_oop_encode_return {
    ($value:expr) => {{
        #[allow(unused_imports)]
        use $crate::types::{EncodeClassReturn as _, EncodeSerializeReturn as _};
        let __napi_oop_rv = $crate::types::ReturnValue::new($value);
        (&__napi_oop_rv).napi_oop_encode()
    }};
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
        let token = self.wire_token();
        let mut m = s.serialize_map(Some(1))?;
        m.serialize_entry(EXTERNAL_KEY, &token)?;
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
        Ok(External {
            token: AtomicU64::new(token),
            value,
        })
    }
}

// In-proc napi bridge: carry the shared `Arc<T>` inside a real napi `External`,
// so the same handle type works when the cdylib is loaded directly by Node. The
// payload type is always `Arc<T>`, keeping napi's type-tag check consistent
// across construction and read-back.
impl<T: Send + Sync + 'static> TypeName for External<T> {
    fn type_name() -> &'static str {
        "External"
    }
    fn value_type() -> napi::ValueType {
        napi::ValueType::External
    }
}

impl<T: Send + Sync + 'static> ValidateNapiValue for External<T> {}

impl<T: Send + Sync + 'static> FromNapiValue for External<T> {
    unsafe fn from_napi_value(env: sys::napi_env, napi_val: sys::napi_value) -> napi::Result<Self> {
        // napi v3's owned `External<T>` is `ToNapiValue`-only; decoding a JS
        // external goes through `FromNapiRef`, which lends a `&'static External`.
        // The external wraps `Arc<T>` (External derefs to its payload), so we
        // clone the `Arc` out to own a shared handle.
        let real = unsafe {
            <napi_bp::External<Arc<T>> as napi_bp::FromNapiRef>::from_napi_ref(env, napi_val)?
        };
        let value: Arc<T> = (**real).clone();
        Ok(External {
            token: AtomicU64::new(0),
            value,
        })
    }
}

impl<T: Send + Sync + 'static> ToNapiValue for External<T> {
    unsafe fn to_napi_value(env: sys::napi_env, val: Self) -> napi::Result<sys::napi_value> {
        unsafe { napi_bp::External::<Arc<T>>::to_napi_value(env, napi_bp::External::new(val.value)) }
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
        let back = from_wire::<BigInt>(v).unwrap();
        assert_eq!(back.words, big.words);
        assert!(!back.sign_bit);
    }

    #[test]
    fn bigint_round_trips_with_full_precision_and_sign() {
        // Multi-word magnitudes and negatives must survive the wire losslessly,
        // not be truncated to a single u64 — matching the in-proc napi door.
        let cases = [
            BigInt {
                sign_bit: false,
                words: vec![0],
            },
            BigInt {
                sign_bit: false,
                words: vec![u64::MAX],
            },
            BigInt {
                sign_bit: true,
                words: vec![1],
            },
            BigInt {
                sign_bit: false,
                words: vec![0, 1], // 2^64
            },
            BigInt {
                sign_bit: true,
                words: vec![7, 0, 0xDEAD_BEEF], // large negative, 3 words
            },
            BigInt {
                sign_bit: false,
                words: vec![u64::MAX, u64::MAX, u64::MAX],
            },
        ];
        for big in cases {
            let v = to_wire(&big).unwrap();
            // Wide values must travel as a msgpack ext, not a plain integer.
            if big.words.iter().filter(|&&w| w != 0).count() > 1 || big.words[0] > i64::MAX as u64 {
                assert!(
                    matches!(v, rmpv::Value::Ext(BIGINT_EXT_TYPE, _)),
                    "wide bigint should be an ext: {big:?} -> {v:?}"
                );
            }
            let back = from_wire::<BigInt>(v).unwrap();
            assert_eq!(back, big, "bigint round trip mismatch");
        }
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
        let wire = to_wire(&e).unwrap();
        let token = e.token.load(Ordering::Relaxed);
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
