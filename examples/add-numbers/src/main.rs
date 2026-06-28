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
use napi_oop_macro::napi;

/// Adds two numbers and returns the result.
#[napi]
pub fn add_numbers(a: i32, b: i32) -> i32 {
    a + b
}

fn main() {
    let result = if std::env::var_os(SOCKET_ENV).is_some() {
        // Spawned as a child: connect back to the parent and serve.
        serve_from_env()
    } else {
        // Parent: the child command to spawn is the rest of the argv.
        let child: Vec<String> = std::env::args().skip(1).collect();
        if child.is_empty() {
            eprintln!(
                "usage: {} <child-command...>   (or set {SOCKET_ENV} to run as a child)",
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
