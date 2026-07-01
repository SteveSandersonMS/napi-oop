//! Dual-mode `Promise<T>`, mirroring napi-rs's `Promise<T>`.
//!
//! A `#[napi]` fn (or, in practice, a `ThreadsafeFunction`'s `Return` type)
//! may name `Promise<T>` to await the JS side's resolved value. napi-rs's
//! `Promise<T>` wraps a live JS promise and can only be *received* from JS
//! (it is never passed back), so it appears only as a parameter or as a
//! callback return type — never as a `#[napi]` return value.
//!
//! One annotated source compiles for both doors:
//! - **In-process** (real napi ABI): [`FromNapiValue`] wraps the real
//!   `napi::Promise<T>`; awaiting delegates to it. Identical to vanilla napi.
//! - **Out-of-process** (wire): the peer resolves the JS promise on its event
//!   loop and sends the *already-resolved* value over the socket, decoded via
//!   serde ([`Deserialize`]). Awaiting yields that value immediately.
//!
//! Both doors expose the same `impl Future<Output = Result<T>>`, so source
//! written for vanilla napi (`let v = promise.await?;`) compiles and behaves
//! identically regardless of how the cdylib was loaded.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use napi::bindgen_prelude::{FromNapiValue, Promise as RealPromise, TypeName, ValidateNapiValue};
use napi::sys;

/// The backing of a [`Promise`]: a live JS promise on the in-process napi path,
/// or an already-resolved value received over the out-of-process wire.
enum Inner<T: FromNapiValue + 'static> {
    /// In-process: a real napi promise wrapping a live JS promise.
    Real(RealPromise<T>),
    /// Out-of-process: the peer already resolved the promise; carry the value.
    Resolved(Option<T>),
}

/// A dual-mode analogue of napi-rs's [`napi::bindgen_prelude::Promise`]. Received
/// from JS (in-proc) or decoded from the wire (out-of-proc); awaited to obtain
/// the resolved value. Never passed back to JS, matching napi-rs.
pub struct Promise<T: FromNapiValue + 'static> {
    inner: Inner<T>,
}

impl<T: FromNapiValue + 'static> Promise<T> {
    /// Build an already-resolved promise from a value decoded off the wire.
    /// Used by the [`serde::Deserialize`] impl; not part of the user surface.
    #[doc(hidden)]
    pub fn __resolved(value: T) -> Self {
        Self {
            inner: Inner::Resolved(Some(value)),
        }
    }
}

// In-proc napi bridge: decode a JS promise into the real-backed variant so the
// same `Promise<T>` type works when the cdylib is loaded by Node directly.
impl<T: FromNapiValue + 'static> TypeName for Promise<T> {
    fn type_name() -> &'static str {
        "Promise"
    }
    fn value_type() -> napi::ValueType {
        napi::ValueType::Object
    }
}

impl<T: FromNapiValue + 'static> ValidateNapiValue for Promise<T> {
    unsafe fn validate(
        env: sys::napi_env,
        napi_val: sys::napi_value,
    ) -> napi::Result<sys::napi_value> {
        unsafe { RealPromise::<T>::validate(env, napi_val) }
    }
}

// Safe to send across threads for the same reason napi's own `Promise<T>` is:
// the underlying receiver is `Send` when `T` is.
unsafe impl<T: FromNapiValue + Send + 'static> Send for Promise<T> {}

impl<T: FromNapiValue + 'static> FromNapiValue for Promise<T> {
    unsafe fn from_napi_value(env: sys::napi_env, napi_val: sys::napi_value) -> napi::Result<Self> {
        let real = unsafe { RealPromise::<T>::from_napi_value(env, napi_val)? };
        Ok(Self {
            inner: Inner::Real(real),
        })
    }
}

/// Decode a wire-carried, already-resolved promise: the peer resolved the JS
/// promise on its event loop, so the wire holds the resolved `T` directly.
impl<'de, T> serde::Deserialize<'de> for Promise<T>
where
    T: FromNapiValue + serde::Deserialize<'de> + 'static,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = T::deserialize(deserializer)?;
        Ok(Self::__resolved(value))
    }
}

impl<T: FromNapiValue + 'static> Future for Promise<T> {
    type Output = napi::Result<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Safety: we never move `T` out of `Inner` except by `Option::take` on the
        // already-resolved (non-self-referential) variant, and the real promise is
        // polled in place through a re-pin, mirroring how napi's own future is driven.
        let this = unsafe { self.get_unchecked_mut() };
        match &mut this.inner {
            Inner::Real(real) => {
                let pinned = unsafe { Pin::new_unchecked(real) };
                pinned.poll(cx)
            }
            Inner::Resolved(value) => Poll::Ready(Ok(value
                .take()
                .expect("napi-oop Promise polled to completion more than once"))),
        }
    }
}
