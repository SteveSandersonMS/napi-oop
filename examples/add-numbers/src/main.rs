//! The `add-numbers` example provider application.
//!
//! Declares the `#[napi]` functions and owns its entrypoint. The provider role
//! (hosting the functions) is independent of which process is the parent:
//!
//! - **Child** (another process spawned us): `NAPI_OOP_SOCKET` is set in the
//!   environment, so connect to it and serve.
//! - **Parent**: spawn the command given on the argv (e.g. `node dist/main.js`)
//!   as the child, passing it a freshly generated socket path, then serve.

use std::process::Command;

use napi_oop::bootstrap::SOCKET_ENV;
use napi_oop::provider::{serve_from_env, spawn_and_serve};
use napi::napi;

/// Adds two numbers and returns the result.
#[napi]
pub fn add_numbers(a: i32, b: i32) -> i32 {
    a + b
}

/// Multiplies two numbers after an async delay — demonstrates an `async fn`,
/// which surfaces as `Promise<number>` on TS in both binding modes. Concurrent
/// calls overlap their delays rather than running serially.
#[napi]
pub async fn multiply_slow(a: i32, b: i32) -> i32 {
    std::thread::sleep(std::time::Duration::from_millis(200));
    a * b
}

/// Adds each number, invoking the JS callback once per element with the running
/// total. Demonstrates a fire-and-forget callback: the closure notifies Node but
/// expects nothing back, matching napi's ThreadsafeFunction.
#[napi]
pub fn sum_each(values: Vec<i32>, on_step: impl Fn(i32)) -> i32 {
    let mut total = 0;
    for v in values {
        total += v;
        on_step(total);
    }
    total
}

/// Same as `sum_each`, but takes an explicit `ThreadsafeFunction<i32>` — the
/// other callback form napi supports. Stored, then fired via `.call`.
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

/// Returns an opaque handle (`External`) to a Rust value. Top-level externals
/// are tracked by JS and released on GC; nesting one in a struct is rejected.
#[napi]
pub fn make_counter(start: i32) -> napi::External<i32> {
    napi::External::new(start)
}

/// Reads a value back from a handle previously returned by `make_counter`.
#[napi]
pub fn read_counter(handle: napi::External<i32>) -> i32 {
    handle.cloned().unwrap_or(-1)
}

fn main() {
    let mut argv = std::env::args().skip(1);
    let first = argv.next();

    if first.as_deref() == Some("--emit-manifest") {
        // Codegen mode: print the type manifest the TS generator consumes.
        println!("{}", napi_oop::manifest::manifest_json());
        return;
    }

    let result = if std::env::var_os(SOCKET_ENV).is_some() {
        // Spawned as a child: connect back to the parent and serve.
        serve_from_env()
    } else {
        // Parent: the child command to spawn is the rest of the argv.
        let mut child: Vec<String> = first.into_iter().collect();
        child.extend(argv);
        if child.is_empty() {
            eprintln!(
                "usage: {} <child-command...>   (or set {SOCKET_ENV} to run as a child, \
                 or --emit-manifest to print types)",
                prog()
            );
            std::process::exit(2);
        }
        let mut command = Command::new(&child[0]);
        command.args(&child[1..]);
        spawn_and_serve(command)
    };

    if let Err(e) = result {
        eprintln!("[provider] error: {e}");
        std::process::exit(1);
    }
}

fn prog() -> String {
    std::env::args().next().unwrap_or_else(|| "add-numbers-provider".into())
}
