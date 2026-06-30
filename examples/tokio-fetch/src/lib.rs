//! The `tokio-fetch` example, built as a single **dual-ABI cdylib**.
//!
//! Demonstrates that real `tokio` futures (here `tokio::time::sleep`) drive
//! correctly through napi-oop: the crate's `tokio` feature makes `block_on` use
//! a shared multi-thread tokio runtime, so the reactor is live. The async fn
//! surfaces on TS as `Promise<T>`; concurrent calls overlap on the runtime.
//!
//! The same `#[napi]` source serves two hosting modes from one artifact:
//! - Node loads it directly as a normal in-process napi addon (real napi ABI).
//! - The thin `tokio-fetch-provider` exe dlopens it and calls
//!   [`tokio_fetch_provider_main`] to serve a Node child out-of-process over
//!   napi-oop. The provider role is independent of which process is the parent:
//!   - **Child** (`NAPI_OOP_SOCKET` set): connect back to the parent and serve.
//!   - **Parent**: spawn the command given on argv (e.g. `node dist/main.js`),
//!     passing it a freshly generated socket path, then serve.

use std::process::Command;
use std::time::Duration;

use napi::napi;
use napi_oop::bootstrap::SOCKET_ENV;
use napi_oop::provider::{serve_from_env, spawn_and_serve};

/// Asynchronously "fetches" a value after a tokio-driven delay, returning the
/// length of the simulated response. Uses a genuine tokio timer, so it surfaces
/// as `Promise<number>` on TS and concurrent calls overlap their delays.
#[napi]
pub async fn fetch_len(url: String) -> u32 {
    tokio::time::sleep(Duration::from_millis(200)).await;
    url.len() as u32
}

/// Out-of-process provider entry, exported for the thin `tokio-fetch-provider`
/// exe to dlopen and call. It runs in the host's own process, so it reads
/// `argv`/env directly — serving an existing socket (`NAPI_OOP_SOCKET` set, the
/// child case), spawning and serving a child command from argv (the parent
/// case), or emitting the manifest. Returns the process exit code for the host.
///
/// Node's in-process `require()` uses the napi addon door
/// (`napi_register_module_v1`) instead and never calls this.
#[no_mangle]
pub extern "C" fn tokio_fetch_provider_main() -> i32 {
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
                "usage: tokio-fetch-provider <child-command...>   (or set {SOCKET_ENV} to run \
                 as a child, or --emit-manifest to print types)"
            );
            return 2;
        }
        let mut command = Command::new(&child[0]);
        command.args(&child[1..]);
        spawn_and_serve(command)
    };

    if let Err(e) = result {
        eprintln!("[tokio-fetch-provider] error: {e}");
        return 1;
    }
    0
}
