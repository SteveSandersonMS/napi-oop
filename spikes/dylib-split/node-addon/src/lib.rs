//! Thin Node-API wrapper. The ONLY artifact with a Node dependency.
//! Each export is a forwarding shim into the shared core via the native Rust ABI
//! (no copy, no serialization) — the heavy logic lives once inside `spike_core`.

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
