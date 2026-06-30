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

/// The hard case: a callback crossing the boundary. The core knows nothing about
/// how the callback is implemented (napi ThreadsafeFunction, a socket round-trip,
/// or a plain closure) — it just takes a neutral `Box<dyn Fn(..) + Send>` and
/// invokes it. It calls from a SPAWNED THREAD on purpose: that is the realistic
/// shape (work happening off the host's main thread) and exactly what each
/// wrapper's callback machinery is designed to bridge.
pub fn with_progress(n: i32, cb: Box<dyn Fn(i32) + Send>) -> i32 {
    let handle = std::thread::spawn(move || {
        for i in 1..=n {
            cb(i);
        }
    });
    handle.join().unwrap();
    n * 10
}

/// Async business logic living in the core. A future is just a value, so it
/// crosses the native-ABI dylib boundary like any other; each wrapper drives it
/// in its own way (a JS Promise in the addon, a blocking executor in the bin).
pub async fn slow_add(a: i32, b: i32) -> i32 {
    a + b
}
