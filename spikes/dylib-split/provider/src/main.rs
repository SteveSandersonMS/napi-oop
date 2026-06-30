//! Thin standalone wrapper (stand-in for a native-host provider executable).
//! Same shared core, a different host. No Node dependency at all.
//! Prints results in a parseable form so the orchestrator can assert on them.

use std::sync::{Arc, Mutex};

#[cfg(not(target_env = "musl"))]
use spike_core_dyn as shared;
#[cfg(target_env = "musl")]
use spike_core as shared;

fn main() {
    println!("add={}", shared::add(2, 3));
    println!("reverse={:?}", shared::reverse_bytes(vec![1, 2, 3]));
    println!("greeting={}", shared::greeting("provider"));

    // Adapts a plain Rust closure (stand-in for a napi-oop socket callback) to the
    // SAME neutral core boundary the napi addon targets. The core is unchanged.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let pret = shared::with_progress(3, Box::new(move |i| sink.lock().unwrap().push(i)));
    println!("progress_ret={}", pret);
    println!("progress_seen={:?}", *seen.lock().unwrap());

    // Drive the core's async fn with a minimal blocking executor (no deps).
    println!("slow_add={}", block_on(shared::slow_add(20, 22)));
}

/// Tiny executor: polls a future to completion on the current thread. Sufficient
/// for the spike's already-ready future; a real provider would use its runtime.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::pin::pin;
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

    fn noop(_: *const ()) {}
    fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(std::ptr::null(), &VTABLE)
    }
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);

    let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
    let mut cx = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
            return value;
        }
        std::thread::yield_now();
    }
}
