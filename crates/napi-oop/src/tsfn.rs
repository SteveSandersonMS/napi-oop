//! Out-of-process `ThreadsafeFunction`, mirroring napi-rs's type of the same
//! name. A `#[napi]` fn may take a `ThreadsafeFunction<T>` param to keep a JS
//! callback past the call and fire it later, from any thread. Like napi, calls
//! are fire-and-forget (non-blocking): they queue onto the peer's event loop.
//!
//! The `impl Fn(..)` sugar and this explicit type are the two callback forms
//! napi-rs supports; the macro recognises both and decodes the same wire handle.

use std::marker::PhantomData;
use std::sync::Arc;

use serde::Serialize;

use crate::codec::HandleId;
use crate::registry::Callbacks;

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
/// calls are always queued, so both modes behave the same here; the variant is
/// accepted for source compatibility with napi-rs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadsafeFunctionCallMode {
    /// Queue even if the buffer is full (napi default; always succeeds here).
    NonBlocking,
    /// Block until queued — a no-op distinction out-of-process.
    Blocking,
}

/// A peer-held JS callback that can be stored and invoked later from any thread.
/// Construct one only from generated glue. Cheap to clone; firing is one-way.
/// The peer handle is released when the last clone drops.
///
/// The generic arity mirrors napi-rs's `ThreadsafeFunction`, which carries a
/// pile of type/const parameters (return type, call-args type, error-status
/// type, callee-handled / weak flags, max-queue-size) to tune the in-process
/// N-API call. Out-of-process only the payload type `T` matters — calls are
/// always queued, fire-and-forget — so every other parameter is a defaulted
/// phantom kept purely for source compatibility, letting the 1-, 5-, and
/// 7-argument forms in existing source all resolve.
pub struct ThreadsafeFunction<
    T,
    Return = (),
    CallJsBackArgs = (),
    ErrorStatus = (),
    const CALLEE_HANDLED: bool = true,
    const WEAK: bool = false,
    const MAX_QUEUE_SIZE: usize = 0,
> {
    inner: Arc<CallbackHandle>,
    _marker: PhantomData<fn(T, Return, CallJsBackArgs, ErrorStatus)>,
}

impl<T, R, C, S, const A: bool, const W: bool, const M: usize>
    ThreadsafeFunction<T, R, C, S, A, W, M>
{
    /// Build from a decoded handle and the shared callback sink. Called by the
    /// `#[napi]` macro; not part of the user surface.
    #[doc(hidden)]
    pub fn __new(handle: HandleId, sink: Arc<dyn Callbacks>) -> Self {
        Self { inner: CallbackHandle::new(handle, sink), _marker: PhantomData }
    }
}

impl<T: Serialize, R, C, S, const A: bool, const W: bool, const M: usize>
    ThreadsafeFunction<T, R, C, S, A, W, M>
{
    /// Fire the callback with `value`. Non-blocking and result-less, matching
    /// napi's default: the value is queued to the peer's event loop.
    pub fn call(&self, value: T, _mode: ThreadsafeFunctionCallMode) {
        if let Ok(arg) = crate::wire::to_wire(&value) {
            self.inner.invoke(vec![arg]);
        }
    }
}

impl<T, R, C, S, const A: bool, const W: bool, const M: usize> Clone
    for ThreadsafeFunction<T, R, C, S, A, W, M>
{
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner), _marker: PhantomData }
    }
}
