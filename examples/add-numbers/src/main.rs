//! The `add-numbers` example application.
//!
//! This is an *application* using the `napi-oop` library out-of-process. It
//! declares its `#[napi]` functions (identical to an in-process napi build) and
//! provides its own entrypoint: parse `<connect|listen> <socket-path>` and hand
//! the connection to the library's provider runtime, which handshakes and serves
//! calls routed to the registered functions.

use napi_oop_macro::napi;

/// Adds two numbers and returns the result.
#[napi]
pub fn add_numbers(a: i32, b: i32) -> i32 {
    a + b
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: {} <connect|listen> <socket-path>", prog(&args));
        std::process::exit(2);
    }

    let path = &args[2];
    let result = match args[1].as_str() {
        "connect" => napi_oop::provider::connect_and_serve(path),
        "listen" => napi_oop::provider::listen_and_serve(path),
        other => {
            eprintln!("unknown mode: {other} (expected `connect` or `listen`)");
            std::process::exit(2);
        }
    };

    if let Err(e) = result {
        eprintln!("[provider] error: {e}");
        std::process::exit(1);
    }
}

fn prog(args: &[String]) -> &str {
    args.first().map(String::as_str).unwrap_or("add-numbers")
}
