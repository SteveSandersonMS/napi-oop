//! Thin standalone wrapper (stand-in for a native-host provider executable).
//! Same shared core, a different host. No Node dependency at all.
//! Prints results in a parseable form so the orchestrator can assert on them.

#[cfg(not(target_env = "musl"))]
use spike_core_dyn as shared;
#[cfg(target_env = "musl")]
use spike_core as shared;

fn main() {
    println!("add={}", shared::add(2, 3));
    println!("reverse={:?}", shared::reverse_bytes(vec![1, 2, 3]));
    println!("greeting={}", shared::greeting("provider"));
}
