//! Thin Node-API wrapper. The ONLY artifact with a Node dependency.
//! Each export is a forwarding shim into the shared core via the native Rust ABI
//! (no copy, no serialization) — the heavy logic lives once inside `spike_core`.

use napi::threadsafe_function::{ErrorStrategy, ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;

#[cfg(not(target_env = "musl"))]
use spike_core_dyn as shared;
#[cfg(target_env = "musl")]
use spike_core as shared;

#[napi]
pub fn add(a: i32, b: i32) -> i32 {
    shared::add(a, b)
}

#[napi]
pub fn reverse_bytes(input: Vec<u8>) -> Vec<u8> {
    shared::reverse_bytes(input)
}

#[napi]
pub fn greeting(name: String) -> String {
    shared::greeting(&name)
}

// Adapts a JS function (delivered as a napi ThreadsafeFunction, callable from any
// thread) down to the core's neutral `Box<dyn Fn(i32) + Send>`. The core never
// sees napi; this wrapper is the only place that knows about ThreadsafeFunction.
#[napi]
pub fn with_progress(n: i32, callback: ThreadsafeFunction<i32, ErrorStrategy::Fatal>) -> i32 {
    shared::with_progress(
        n,
        Box::new(move |i| {
            callback.call(i, ThreadsafeFunctionCallMode::NonBlocking);
        }),
    )
}

// Surfaces the core's async fn as a JS Promise.
#[napi]
pub async fn slow_add(a: i32, b: i32) -> i32 {
    shared::slow_add(a, b).await
}
