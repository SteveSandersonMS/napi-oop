//! Out-of-process shims for the napi-rs surface that isn't a value-carrying type
//! (those live in [`crate::types`]). These mirror napi-rs's `Error`, `Status`,
//! `Env`, `Object`, `Utf16String`, `AsyncBlockBuilder`, and the `spawn` helpers
//! so unmodified `#[napi]` source compiles on the `napi::` path out-of-process.
//!
//! Source compatibility is the contract: the signatures match napi-rs closely
//! enough that the same code builds, while the behavior is adapted to the
//! cross-process world (e.g. `Object` is a plain value map serialized over the
//! wire rather than a live JS handle).

use std::fmt;
use std::ops::Deref;

use serde::{Deserialize, Serialize};

/// Drop-in for `napi::Result`. Mirrors napi-rs's alias so `napi::Result<T>`
/// (one type argument) resolves out-of-process exactly as in-process.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Mirrors napi-rs's `Status`. Out-of-process there is no live N-API call to
/// fail, so the variant is informational — carried inside [`Error`] and accepted
/// as the error-status type parameter of [`crate::ThreadsafeFunction`]. The
/// variant set matches napi-rs so `Status::GenericFailure` etc. resolve.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Status {
    Ok,
    InvalidArg,
    ObjectExpected,
    StringExpected,
    NameExpected,
    FunctionExpected,
    NumberExpected,
    BooleanExpected,
    ArrayExpected,
    #[default]
    GenericFailure,
    PendingException,
    Cancelled,
    EscapeCalledTwice,
    HandleScopeMismatch,
    CallbackScopeMismatch,
    QueueFull,
    Closing,
    BigintExpected,
    DateExpected,
    ArrayBufferExpected,
    DetachableArraybufferExpected,
    WouldDeadlock,
    NoExternalBuffersAllowed,
    Unknown,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

/// Mirrors napi-rs's `Error`. In-process this wraps an N-API status + message
/// that becomes a thrown JS exception; out-of-process the dispatcher turns a
/// returned `Err` into an error reply carrying the message, so only the
/// `reason` string crosses the boundary. `status` is retained for source
/// compatibility (`Error::new(Status::…, …)`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub status: Status,
    pub reason: String,
}

impl Error {
    /// Build an error from a human-readable reason, defaulting the status to
    /// `GenericFailure` — the common napi-rs constructor.
    pub fn from_reason<T: Into<String>>(reason: T) -> Self {
        Error {
            status: Status::GenericFailure,
            reason: reason.into(),
        }
    }

    /// Build an error with an explicit status and reason.
    pub fn new<T: Into<String>>(status: Status, reason: T) -> Self {
        Error {
            status,
            reason: reason.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.reason.is_empty() {
            write!(f, "{}", self.status)
        } else {
            write!(f, "{}", self.reason)
        }
    }
}

impl std::error::Error for Error {}

impl From<String> for Error {
    fn from(reason: String) -> Self {
        Error::from_reason(reason)
    }
}

impl From<&str> for Error {
    fn from(reason: &str) -> Self {
        Error::from_reason(reason)
    }
}

/// Mirrors napi-rs's `Env`. Out-of-process there is no JS environment to borrow,
/// so this is an opaque zero-sized token: it satisfies signatures that thread an
/// `Env`/`&Env` through (e.g. building an [`Object`]) without carrying state.
#[derive(Clone, Copy, Debug, Default)]
pub struct Env;

/// Mirrors napi-rs's `Object`. In-process this is a live JS object handle; here
/// it is a plain ordered map of already-serialized values that rides the wire as
/// a MessagePack map. `set` takes any `Serialize` value and `get` any
/// `DeserializeOwned` one, matching the by-value object flows that work
/// cross-process. The lifetime parameter mirrors napi-rs's `Object<'env>` so
/// signatures line up; it carries no borrow here.
#[derive(Clone, Debug, Default)]
pub struct Object<'env> {
    entries: Vec<(String, rmpv::Value)>,
    _env: std::marker::PhantomData<&'env ()>,
}

impl<'env> Object<'env> {
    /// Create an empty object. The `Env` is accepted for source compatibility.
    pub fn new(_env: &'env Env) -> Result<Self> {
        Ok(Object {
            entries: Vec::new(),
            _env: std::marker::PhantomData,
        })
    }

    /// Set a property to any serializable value (last write wins per key).
    pub fn set<V: Serialize>(&mut self, key: impl Into<String>, value: V) -> Result<()> {
        let key = key.into();
        let value = crate::wire::to_wire(&value).map_err(|e| Error::from_reason(e.to_string()))?;
        if let Some(slot) = self.entries.iter_mut().find(|(k, _)| *k == key) {
            slot.1 = value;
        } else {
            self.entries.push((key, value));
        }
        Ok(())
    }

    /// Get a property, deserialized to `V`, or `None` if absent.
    pub fn get<V: serde::de::DeserializeOwned>(&self, key: &str) -> Result<Option<V>> {
        match self.entries.iter().find(|(k, _)| k == key) {
            Some((_, v)) => crate::wire::from_wire(v.clone())
                .map(Some)
                .map_err(|e| Error::from_reason(e.to_string())),
            None => Ok(None),
        }
    }
}

impl Serialize for Object<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = s.serialize_map(Some(self.entries.len()))?;
        for (k, v) in &self.entries {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

/// Mirrors napi-rs's `Utf16String`. In-process this preserves raw UTF-16 code
/// units (incl. lone surrogates) across the boundary; over MessagePack a JS
/// string arrives as UTF-8, so this decodes to code units (lossy only for
/// unpaired surrogates, which can't survive a UTF-8 hop anyway). Derefs to
/// `[u16]` so `&value` passes to `&[u16]` APIs unchanged.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Utf16String(Vec<u16>);

impl Deref for Utf16String {
    type Target = [u16];
    fn deref(&self) -> &[u16] {
        &self.0
    }
}

impl AsRef<[u16]> for Utf16String {
    fn as_ref(&self) -> &[u16] {
        &self.0
    }
}

impl From<&str> for Utf16String {
    fn from(s: &str) -> Self {
        Utf16String(s.encode_utf16().collect())
    }
}

impl fmt::Display for Utf16String {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", String::from_utf16_lossy(&self.0))
    }
}

impl Serialize for Utf16String {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&String::from_utf16_lossy(&self.0))
    }
}

impl<'de> Deserialize<'de> for Utf16String {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(Utf16String(s.encode_utf16().collect()))
    }
}

/// Mirrors napi-rs's `AsyncBlockBuilder`, which wraps an async block so it can be
/// turned into a JS `Promise`. Out-of-process the macro already drives async
/// `#[napi]` fns to completion, so this minimal shim exists for source
/// compatibility with the few fns that build an async block by hand: `build`
/// runs the future to completion via [`crate::block_on`].
pub struct AsyncBlockBuilder<F> {
    future: F,
}

impl<F: std::future::Future> AsyncBlockBuilder<F> {
    pub fn new(future: F) -> Self {
        AsyncBlockBuilder { future }
    }

    /// Run the wrapped future to completion. The `Env` is accepted for source
    /// compatibility with napi-rs's `build(env)`.
    pub fn build(self, _env: &Env) -> Result<F::Output> {
        Ok(crate::block_on(self.future))
    }
}

/// Mirrors `napi::bindgen_prelude::spawn`: run a future on the shared async
/// runtime, returning a join handle. Out-of-process this is the runtime's own
/// task spawn (tokio), used by fns that kick off background work.
#[cfg(feature = "tokio")]
pub fn spawn<F>(future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    tokio::task::spawn(future)
}

/// Mirrors `napi::bindgen_prelude::spawn_blocking`: run a blocking closure off
/// the async runtime's worker threads, returning a join handle.
#[cfg(feature = "tokio")]
pub fn spawn_blocking<F, R>(f: F) -> tokio::task::JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    tokio::task::spawn_blocking(f)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{from_wire, to_wire};

    #[test]
    fn error_from_reason_carries_message() {
        let e = Error::from_reason("boom");
        assert_eq!(e.reason, "boom");
        assert_eq!(e.status, Status::GenericFailure);
        assert_eq!(e.to_string(), "boom");
    }

    #[test]
    fn utf16string_round_trips_as_string() {
        let s = Utf16String::from("héllo");
        let v = to_wire(&s).unwrap();
        assert!(matches!(v, rmpv::Value::String(_)));
        assert_eq!(from_wire::<Utf16String>(v).unwrap(), s);
        // Derefs to code units for `&[u16]` APIs.
        assert_eq!(&*s, "héllo".encode_utf16().collect::<Vec<_>>().as_slice());
    }

    #[test]
    fn object_serializes_as_map() {
        let env = Env;
        let mut o = Object::new(&env).unwrap();
        o.set("requestId", 7u32).unwrap();
        o.set("name", "fetch").unwrap();
        let v = to_wire(&o).unwrap();
        assert!(matches!(v, rmpv::Value::Map(_)));
        assert_eq!(o.get::<u32>("requestId").unwrap(), Some(7));
        assert_eq!(o.get::<String>("name").unwrap().as_deref(), Some("fetch"));
        assert_eq!(o.get::<u32>("missing").unwrap(), None);
    }
}
