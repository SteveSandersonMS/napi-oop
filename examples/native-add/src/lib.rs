use napi::napi;

/// Same source shape as the out-of-proc example, but built in-proc as a native
/// `.node`. The facade routes `#[napi]` to real napi-rs here.
#[napi]
pub fn add_numbers(a: i32, b: i32) -> i32 {
    a + b
}

#[napi]
pub fn greet(name: String) -> String {
    format!("Hello, {name}!")
}
