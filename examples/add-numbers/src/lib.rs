//! The `add-numbers` example, built as a single **dual-ABI cdylib**.
//!
//! The same `#[napi]` source serves two hosting modes from one artifact:
//! - Node loads it directly as a normal in-process napi addon (real napi ABI).
//! - The thin `add-numbers-provider` exe dlopens it and calls
//!   [`add_numbers_provider_main`] to serve a Node child out-of-process over
//!   napi-oop. The provider role (hosting the functions) is independent of which
//!   process is the parent:
//!   - **Child** (another process spawned us): `NAPI_OOP_SOCKET` is set, so
//!     connect to it and serve.
//!   - **Parent**: spawn the command given on argv (e.g. `node dist/main.js`) as
//!     the child, passing it a freshly generated socket path, then serve.

use std::process::Command;

use napi::napi;
use napi_oop::bootstrap::SOCKET_ENV;
use napi_oop::provider::{serve_from_env, spawn_and_serve};

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
/// other callback form napi supports. Being `CalleeHandled` (the napi default),
/// it delivers `(err, value)` to JS, so the running total is passed as `Ok`.
#[napi]
pub fn sum_each_tsfn(values: Vec<i32>, on_step: napi::ThreadsafeFunction<i32>) -> i32 {
    use napi::ThreadsafeFunctionCallMode::NonBlocking;
    let mut total = 0;
    for v in values {
        total += v;
        on_step.call(Ok(total), NonBlocking);
    }
    total
}

/// Out-of-process provider entry, exported for the thin `add-numbers-provider`
/// exe to dlopen and call. It runs in the host's own process, so it reads
/// `argv`/env directly — serving an existing socket (`NAPI_OOP_SOCKET` set, the
/// child case), spawning and serving a child command from argv (the parent
/// case), or emitting the manifest. Returns the process exit code for the host.
///
/// Node's in-process `require()` uses the napi addon door
/// (`napi_register_module_v1`) instead and never calls this.
#[no_mangle]
pub extern "C" fn add_numbers_provider_main() -> i32 {
    let mut argv = std::env::args().skip(1);
    let first = argv.next();

    if first.as_deref() == Some("--emit-manifest") {
        // Codegen mode: print the type manifest the TS generator consumes.
        println!("{}", napi_oop::manifest::manifest_json());
        return 0;
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
                "usage: add-numbers-provider <child-command...>   (or set {SOCKET_ENV} to run \
                 as a child, or --emit-manifest to print types)"
            );
            return 2;
        }
        let mut command = Command::new(&child[0]);
        command.args(&child[1..]);
        spawn_and_serve(command)
    };

    if let Err(e) = result {
        eprintln!("[add-numbers-provider] error: {e}");
        return 1;
    }
    0
}
