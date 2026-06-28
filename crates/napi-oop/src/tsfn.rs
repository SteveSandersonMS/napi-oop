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
pub struct ThreadsafeFunction<T: Serialize> {
    handle: HandleId,
    sink: Arc<dyn Callbacks>,
    _marker: PhantomData<fn(T)>,
}

impl<T: Serialize> ThreadsafeFunction<T> {
    /// Build from a decoded handle and the shared callback sink. Called by the
    /// `#[napi]` macro; not part of the user surface.
    #[doc(hidden)]
    pub fn __new(handle: HandleId, sink: Arc<dyn Callbacks>) -> Self {
        Self { handle, sink, _marker: PhantomData }
    }

    /// Fire the callback with `value`. Non-blocking and result-less, matching
    /// napi's default: the value is queued to the peer's event loop.
    pub fn call(&self, value: T, _mode: ThreadsafeFunctionCallMode) {
        let arg = match crate::wire::to_wire(&value) {
            Ok(v) => v,
            Err(_) => return,
        };
        self.sink.invoke(self.handle, vec![arg]);
    }
}

impl<T: Serialize> Clone for ThreadsafeFunction<T> {
    fn clone(&self) -> Self {
        Self { handle: self.handle, sink: Arc::clone(&self.sink), _marker: PhantomData }
    }
}
