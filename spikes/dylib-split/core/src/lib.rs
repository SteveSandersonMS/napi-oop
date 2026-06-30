//! Stand-in for the "99% business logic" that should be compiled exactly once.
//!
//! It is deliberately Node-free and Node-API-free: it knows nothing about how it
//! will be hosted. The public surface IS the ABI consumed by the wrappers.

/// Trivial value-in/value-out call.
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

/// Exercises an owned heap type crossing the dylib boundary (Vec<u8>),
/// to confirm allocation/free works across the shared-std boundary.
pub fn reverse_bytes(mut v: Vec<u8>) -> Vec<u8> {
    v.reverse();
    v
}

/// Exercises String crossing the boundary plus formatting in the core.
pub fn greeting(name: &str) -> String {
    format!("hello, {name}")
}
