//! Out-of-process `ThreadsafeFunction`, mirroring napi-rs's type of the same
//! name. A `#[napi]` fn may take a `ThreadsafeFunction<T>` param to keep a JS
//! callback past the call and fire it later, from any thread. Like napi, calls
//! are fire-and-forget (non-blocking): they queue onto the peer's event loop.
//!
//! The `impl Fn(..)` sugar and this explicit type are the two callback forms
//! napi-rs supports; the macro recognises both and decodes the same wire handle.
//!
//! Faithfulness to napi v3: the wrapper carries the same generic shape as napi's
//! own `ThreadsafeFunction` — including the `CalleeHandled` const flag. A default
//! `ThreadsafeFunction<T>` is `CalleeHandled = true`, so its `call` takes a
//! `Result<T, _>` and delivers `(err, value)` to the JS callback, exactly as
//! vanilla napi does; a `CalleeHandled = false` one takes a bare `T` and delivers
//! just `(value)`. Both doors (in-proc real napi and out-of-proc wire) follow the
//! same convention, so observable behaviour matches traditional napi.

use std::cell::RefCell;
use std::marker::PhantomData;
use std::sync::Arc;

use napi::bindgen_prelude::{
    FromNapiValue, JsValuesTupleIntoVec, TypeName, Unknown, ValidateNapiValue,
};
use napi::sys;
use napi::threadsafe_function::{
    ThreadsafeFunction as RealTsfn, ThreadsafeFunctionCallMode as RealCallMode,
};
use napi::Status;
use serde::Serialize;

use crate::codec::HandleId;
use crate::registry::Callbacks;

thread_local! {
    /// The callback sink for the call currently being dispatched on this thread.
    /// Set for the duration of a dispatch so that decoding a `ThreadsafeFunction`
    /// argument (which may be hidden behind a type alias the macro can't see)
    /// can reach the sink without the macro threading it through explicitly.
    static CURRENT_CALLBACKS: RefCell<Option<Arc<dyn Callbacks>>> = const { RefCell::new(None) };
}

/// Install `sink` as the current-thread callback sink, restoring the previous one
/// when the returned guard drops. The dispatcher wraps each call in this so that
/// `ThreadsafeFunction`'s `Deserialize` impl can pick the sink up.
#[doc(hidden)]
pub fn push_callbacks(sink: Arc<dyn Callbacks>) -> CallbacksGuard {
    let prev = CURRENT_CALLBACKS.with(|slot| slot.borrow_mut().replace(sink));
    CallbacksGuard(prev)
}

/// Restores the previous current-thread callback sink on drop.
#[doc(hidden)]
pub struct CallbacksGuard(Option<Arc<dyn Callbacks>>);

impl Drop for CallbacksGuard {
    fn drop(&mut self) {
        let prev = self.0.take();
        CURRENT_CALLBACKS.with(|slot| *slot.borrow_mut() = prev);
    }
}

fn current_callbacks() -> Option<Arc<dyn Callbacks>> {
    CURRENT_CALLBACKS.with(|slot| slot.borrow().clone())
}

/// Shared owner of a peer callback handle. Sends `release` once dropped by its
/// last holder, so both callback forms free the JS handle automatically.
pub struct CallbackHandle {
    handle: HandleId,
    sink: Arc<dyn Callbacks>,
}

impl CallbackHandle {
    #[doc(hidden)]
    pub fn new(handle: HandleId, sink: Arc<dyn Callbacks>) -> Arc<Self> {
        Arc::new(Self { handle, sink })
    }

    /// Fire the callback with already-encoded args (fire-and-forget).
    pub fn invoke(&self, args: Vec<rmpv::Value>) {
        self.sink.invoke(self.handle, args);
    }
}

impl Drop for CallbackHandle {
    fn drop(&mut self) {
        self.sink.release(self.handle);
    }
}

/// How a [`ThreadsafeFunction`] call delivers, mirroring napi's enum. Out-of-proc
/// calls are always queued, so both modes behave the same here; in-proc the
/// variant is forwarded to napi's own call mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadsafeFunctionCallMode {
    /// Queue even if the buffer is full (napi default; always succeeds here).
    NonBlocking,
    /// Block until queued — a no-op distinction out-of-process.
    Blocking,
}

impl From<ThreadsafeFunctionCallMode> for RealCallMode {
    fn from(mode: ThreadsafeFunctionCallMode) -> Self {
        match mode {
            ThreadsafeFunctionCallMode::NonBlocking => RealCallMode::NonBlocking,
            ThreadsafeFunctionCallMode::Blocking => RealCallMode::Blocking,
        }
    }
}

/// The backing of a [`ThreadsafeFunction`]: a peer handle on the out-of-process
/// wire path, or a real napi threadsafe function on the in-process napi path.
/// One annotated source produces both; which variant a value carries depends on
/// how the cdylib was loaded (Node directly vs. a thin provider exe).
///
/// The real napi v3 `ThreadsafeFunction` is not `Clone`, so it is held behind an
/// `Arc`: cloning the wrapper shares the one underlying napi handle, which is
/// released when the last clone drops — matching napi's own ref-counted lifetime.
/// `CallJsBackArgs` is forced to `T` (the only shape napi's `FromNapiValue` /
/// `call` impls use), so it is not a separate parameter of `Inner`.
enum Inner<T, R, S, const A: bool, const W: bool, const M: usize>
where
    T: JsValuesTupleIntoVec + 'static,
    R: FromNapiValue + 'static,
    S: AsRef<str> + From<Status>,
{
    /// Out-of-process: the peer's JS callback, fired over the socket.
    Wire(Arc<CallbackHandle>),
    /// In-process: a real napi threadsafe function, fired through N-API.
    Real(Arc<RealTsfn<T, R, T, S, A, W, M>>),
}

impl<T, R, S, const A: bool, const W: bool, const M: usize> Clone for Inner<T, R, S, A, W, M>
where
    T: JsValuesTupleIntoVec + 'static,
    R: FromNapiValue + 'static,
    S: AsRef<str> + From<Status>,
{
    fn clone(&self) -> Self {
        match self {
            Inner::Wire(h) => Inner::Wire(Arc::clone(h)),
            Inner::Real(r) => Inner::Real(Arc::clone(r)),
        }
    }
}

/// A peer-held JS callback that can be stored and invoked later from any thread.
/// Construct one only from generated glue. Cheap to clone; firing is one-way.
/// The peer handle is released when the last clone drops.
///
/// The generic arity mirrors napi-rs's `ThreadsafeFunction`: the payload type
/// `T`, the JS return type `Return`, the callback-args type `CallJsBackArgs`, the
/// `ErrorStatus`, and the `CalleeHandled` / `Weak` / `MaxQueueSize` const flags.
/// `CalleeHandled` is the one that changes the `call` signature (see the two
/// `call` impls below); the others tune the in-process N-API call and are inert
/// out-of-process, where calls are always queued fire-and-forget.
pub struct ThreadsafeFunction<
    T: 'static,
    Return = Unknown<'static>,
    CallJsBackArgs = T,
    ErrorStatus = Status,
    const CALLEE_HANDLED: bool = true,
    const WEAK: bool = false,
    const MAX_QUEUE_SIZE: usize = 0,
>
where
    T: JsValuesTupleIntoVec,
    Return: FromNapiValue + 'static,
    ErrorStatus: AsRef<str> + From<Status>,
{
    inner: Inner<T, Return, ErrorStatus, CALLEE_HANDLED, WEAK, MAX_QUEUE_SIZE>,
    _marker: PhantomData<fn(CallJsBackArgs)>,
}

impl<T, R, C, S, const A: bool, const W: bool, const M: usize>
    ThreadsafeFunction<T, R, C, S, A, W, M>
where
    T: JsValuesTupleIntoVec + 'static,
    R: FromNapiValue + 'static,
    S: AsRef<str> + From<Status>,
{
    /// Build from a decoded handle and the shared callback sink (out-of-process
    /// path). Called by the `#[napi]` macro; not part of the user surface.
    #[doc(hidden)]
    pub fn __new(handle: HandleId, sink: Arc<dyn Callbacks>) -> Self {
        Self {
            inner: Inner::Wire(CallbackHandle::new(handle, sink)),
            _marker: PhantomData,
        }
    }
}

impl<T, R, C, S, const W: bool, const M: usize> ThreadsafeFunction<T, R, C, S, true, W, M>
where
    T: JsValuesTupleIntoVec + Serialize + 'static,
    R: FromNapiValue + 'static,
    S: AsRef<str> + From<Status>,
{
    /// Fire the callback the napi default (`CalleeHandled`) way: the JS callback
    /// receives `(err, value)`. `Ok(v)` delivers `(null, v)`; `Err(e)` delivers
    /// the error as the first argument. The value type is napi's own
    /// `Result<T, Error<S>>`, so source written for vanilla napi (`.call(Ok(v),
    /// mode)`) compiles unchanged. Non-blocking and result-less, matching napi:
    /// the value is queued (out-of-process onto the peer's event loop; in-process
    /// onto the host's). Returns napi's own `Status` — the out-of-process queue
    /// cannot fail synchronously, so it returns `Status::Ok`.
    pub fn call(
        &self,
        value: Result<T, napi::Error<S>>,
        mode: ThreadsafeFunctionCallMode,
    ) -> crate::shim::Status {
        match &self.inner {
            Inner::Wire(handle) => {
                match value {
                    Ok(v) => {
                        if let Ok(arg) = crate::wire::to_wire(&v) {
                            // (null, value): a leading nil error slot mirrors
                            // vanilla napi's CalleeHandled `(err, value)` shape.
                            handle.invoke(vec![rmpv::Value::Nil, arg]);
                        }
                    }
                    Err(err) => {
                        let msg = err.reason.clone();
                        if let Ok(arg) = crate::wire::to_wire(&msg) {
                            handle.invoke(vec![arg]);
                        }
                    }
                }
                crate::shim::Status::Ok
            }
            Inner::Real(real) => real.call(value, mode.into()),
        }
    }
}

impl<T, R, C, S, const W: bool, const M: usize> ThreadsafeFunction<T, R, C, S, false, W, M>
where
    T: JsValuesTupleIntoVec + Serialize + 'static,
    R: FromNapiValue + 'static,
    S: AsRef<str> + From<Status>,
{
    /// Fire the callback the `CalleeHandled = false` way: the JS callback receives
    /// just `(value)`, with no leading error slot. Non-blocking and result-less.
    pub fn call(&self, value: T, mode: ThreadsafeFunctionCallMode) -> crate::shim::Status {
        match &self.inner {
            Inner::Wire(handle) => {
                if let Ok(arg) = crate::wire::to_wire(&value) {
                    handle.invoke(vec![arg]);
                }
                crate::shim::Status::Ok
            }
            Inner::Real(real) => real.call(value, mode.into()),
        }
    }
}

impl<T, R, C, S, const A: bool, const W: bool, const M: usize> Clone
    for ThreadsafeFunction<T, R, C, S, A, W, M>
where
    T: JsValuesTupleIntoVec + 'static,
    R: FromNapiValue + 'static,
    S: AsRef<str> + From<Status>,
{
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            _marker: PhantomData,
        }
    }
}

// In-proc napi bridge: decode a JS function param into the real-backed variant so
// the same `ThreadsafeFunction<T>` type works when the cdylib is loaded by Node.
impl<T, R, C, S, const A: bool, const W: bool, const M: usize> TypeName
    for ThreadsafeFunction<T, R, C, S, A, W, M>
where
    T: JsValuesTupleIntoVec + 'static,
    R: FromNapiValue + 'static,
    S: AsRef<str> + From<Status>,
{
    fn type_name() -> &'static str {
        "Function"
    }
    fn value_type() -> napi::ValueType {
        napi::ValueType::Function
    }
}

impl<T, R, C, S, const A: bool, const W: bool, const M: usize> ValidateNapiValue
    for ThreadsafeFunction<T, R, C, S, A, W, M>
where
    T: JsValuesTupleIntoVec + 'static,
    R: FromNapiValue + 'static,
    S: AsRef<str> + From<Status>,
{
}

impl<T, R, C, S, const A: bool, const W: bool, const M: usize> FromNapiValue
    for ThreadsafeFunction<T, R, C, S, A, W, M>
where
    T: JsValuesTupleIntoVec + 'static,
    R: FromNapiValue + 'static,
    S: AsRef<str> + From<Status>,
{
    unsafe fn from_napi_value(env: sys::napi_env, napi_val: sys::napi_value) -> napi::Result<Self> {
        let real =
            unsafe { RealTsfn::<T, R, T, S, A, W, M>::from_napi_value(env, napi_val)? };
        Ok(Self {
            inner: Inner::Real(Arc::new(real)),
            _marker: PhantomData,
        })
    }
}

/// Decode a `ThreadsafeFunction` from its `{ "__napi_cb": <id> }` wire marker,
/// binding it to the current dispatch's callback sink. This lets a callback
/// parameter decode through the ordinary serde/`from_wire` path even when its
/// type is hidden behind an alias (`type Foo = ThreadsafeFunction<…>`), which
/// the macro can't recognise syntactically.
impl<'de, T, R, C, S, const A: bool, const W: bool, const M: usize> serde::Deserialize<'de>
    for ThreadsafeFunction<T, R, C, S, A, W, M>
where
    T: JsValuesTupleIntoVec + 'static,
    R: FromNapiValue + 'static,
    S: AsRef<str> + From<Status>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct Handle {
            #[serde(rename = "__napi_cb")]
            id: HandleId,
        }
        let handle = Handle::deserialize(deserializer)?;
        let sink = current_callbacks().ok_or_else(|| {
            serde::de::Error::custom(
                "ThreadsafeFunction decoded outside a dispatch scope (no callback sink available)",
            )
        })?;
        Ok(Self::__new(handle.id, sink))
    }
}
