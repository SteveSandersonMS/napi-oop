//! A tokio-based out-of-process provider.
//!
//! Demonstrates that real `tokio` futures (here `tokio::time::sleep`) drive
//! correctly through napi-oop: the crate's `tokio` feature makes `block_on` use
//! a shared multi-thread tokio runtime, so the reactor is live. The async fn
//! surfaces on TS as `Promise<T>`; concurrent calls overlap on the runtime.
//!
//! The provider role and bootstrap are identical to the add-numbers example: a
//! child when `NAPI_OOP_SOCKET` is set, else a parent spawning the given argv.

use std::process::Command;
use std::time::Duration;

use napi_oop::bootstrap::SOCKET_ENV;
use napi_oop::provider::{serve_from_env, spawn_and_serve};
use napi_oop_macro::napi;

/// Asynchronously "fetches" a value after a tokio-driven delay, returning the
/// length of the simulated response. Uses a genuine tokio timer.
#[napi]
pub async fn fetch_len(url: String) -> u32 {
    tokio::time::sleep(Duration::from_millis(200)).await;
    url.len() as u32
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
            eprintln!("usage: tokio-fetch-provider <child-command...> | --emit-manifest");
            std::process::exit(2);
        }
        let mut command = Command::new(&child[0]);
        command.args(&child[1..]);
        spawn_and_serve(command)
    };

    if let Err(e) = result {
        eprintln!("[tokio-provider] error: {e}");
        std::process::exit(1);
    }
}
