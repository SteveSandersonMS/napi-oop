//! E2E test fixture provider. A dedicated set of `#[napi]` functions exercising
//! every cross-process flow napi-oop supports — sync, async, both callback
//! forms, Buffer, BigInt, and External (incl. a slab probe to prove GC release).
//! Kept separate from the examples so the examples stay clean and human-facing.

use std::process::Command;

use napi::napi;
use napi_oop::bootstrap::SOCKET_ENV;
use napi_oop::provider::{serve_from_env, spawn_and_serve};

#[napi]
pub fn add_numbers(a: i32, b: i32) -> i32 {
    a + b
}

#[napi]
pub async fn multiply_slow(a: i32, b: i32) -> i32 {
    std::thread::sleep(std::time::Duration::from_millis(200));
    a * b
}

#[napi]
pub fn sum_each(values: Vec<i32>, on_step: impl Fn(i32)) -> i32 {
    let mut total = 0;
    for v in values {
        total += v;
        on_step(total);
    }
    total
}

#[napi]
pub fn sum_each_tsfn(values: Vec<i32>, on_step: napi::ThreadsafeFunction<i32>) -> i32 {
    use napi::ThreadsafeFunctionCallMode::NonBlocking;
    let mut total = 0;
    for v in values {
        total += v;
        on_step.call(total, NonBlocking);
    }
    total
}

#[napi]
pub fn reverse_bytes(b: napi::Buffer) -> napi::Buffer {
    let mut v = b.to_vec();
    v.reverse();
    napi::Buffer::from(v)
}

#[napi]
pub fn double_big(n: napi::BigInt) -> napi::BigInt {
    napi::BigInt::from(n.words.wrapping_mul(2))
}

#[napi]
pub fn make_counter(start: i32) -> napi::External<i32> {
    napi::External::new(start)
}

#[napi]
pub fn read_counter(handle: napi::External<i32>) -> i32 {
    handle.cloned().unwrap_or(-1)
}

/// Live `External` handles the provider still holds, so the suite can assert
/// GC-collected handles release their slab entry.
#[napi]
pub fn live_counters() -> i32 {
    napi_oop::types::external_slab_len() as i32
}

fn main() {
    let mut argv = std::env::args().skip(1);
    let first = argv.next();
    if first.as_deref() == Some("--emit-manifest") {
        println!("{}", napi_oop::manifest::manifest_json());
        return;
    }
    let result = if std::env::var_os(SOCKET_ENV).is_some() {
        serve_from_env()
    } else {
        let mut child: Vec<String> = first.into_iter().collect();
        child.extend(argv);
        if child.is_empty() {
            eprintln!("usage: e2e-provider <child-command...> (or --emit-manifest)");
            std::process::exit(2);
        }
        let mut command = Command::new(&child[0]);
        command.args(&child[1..]);
        spawn_and_serve(command)
    };
    if let Err(e) = result {
        eprintln!("[e2e-provider] error: {e}");
        std::process::exit(1);
    }
}
