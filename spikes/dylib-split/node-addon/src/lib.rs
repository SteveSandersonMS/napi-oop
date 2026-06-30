//! Thin Node-API wrapper. The ONLY artifact with a Node dependency.
//! Each export is a forwarding shim into the shared core via the native Rust ABI
//! (no copy, no serialization) — the heavy logic lives once inside `spike_core`.

use napi_derive::napi;

#[napi]
pub fn add(a: i32, b: i32) -> i32 {
    spike_core::add(a, b)
}

#[napi]
pub fn reverse_bytes(input: Vec<u8>) -> Vec<u8> {
    spike_core::reverse_bytes(input)
}

#[napi]
pub fn greeting(name: String) -> String {
    spike_core::greeting(&name)
}
