#![deny(clippy::all)]

use napi_derive::napi;

/// Adds two numbers and returns the result.
#[napi]
pub fn add_numbers(a: i32, b: i32) -> i32 {
    a + b
}
