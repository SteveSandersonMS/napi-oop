//! Thin standalone wrapper (stand-in for a native-host provider executable).
//! Same shared core, a different host. No Node dependency at all.
//! Prints results in a parseable form so the orchestrator can assert on them.

fn main() {
    println!("add={}", spike_core::add(2, 3));
    println!("reverse={:?}", spike_core::reverse_bytes(vec![1, 2, 3]));
    println!("greeting={}", spike_core::greeting("provider"));
}
