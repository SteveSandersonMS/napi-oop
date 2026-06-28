//! End-to-end Phase 3 test: the out-of-proc `#[napi]` macro registers functions
//! and the dispatcher routes wire requests to them — driven entirely by serde,
//! with no per-type codec written by hand.

use napi_oop::codec::{Message, Request};
use napi_oop::registry;
use napi_oop::rmpv::Value;
use napi_oop_macro::napi;
use serde::{Deserialize, Serialize};

#[napi]
pub fn add_numbers(a: i32, b: i32) -> i32 {
    a + b
}

#[napi]
pub fn greet(name: String) -> String {
    format!("Hello, {name}!")
}

/// A derived struct crosses the boundary with no bespoke codec — serde alone.
#[derive(Serialize, Deserialize)]
pub struct Point {
    x: i32,
    y: i32,
}

#[napi]
pub fn manhattan(p: Point) -> i32 {
    p.x.abs() + p.y.abs()
}

fn call(function: &str, id: u64, args: Vec<Value>) -> Message {
    registry::dispatch(Request {
        id,
        function: function.to_string(),
        args,
    })
}

#[test]
fn macro_registers_all_functions() {
    let names = registry::registered_names();
    for expected in ["add_numbers", "greet", "manhattan"] {
        assert!(names.contains(&expected.to_string()), "missing {expected}");
    }
}

#[test]
fn dispatches_add_numbers() {
    match call("add_numbers", 1, vec![Value::from(2i64), Value::from(3i64)]) {
        Message::Response(r) => {
            assert_eq!(r.id, 1);
            assert_eq!(r.result.as_i64(), Some(5));
        }
        other => panic!("expected response, got {other:?}"),
    }
}

#[test]
fn dispatches_string_function() {
    match call("greet", 2, vec![Value::from("Ada")]) {
        Message::Response(r) => assert_eq!(r.result.as_str(), Some("Hello, Ada!")),
        other => panic!("expected response, got {other:?}"),
    }
}

#[test]
fn dispatches_derived_struct_arg() {
    let point = Value::Map(vec![
        (Value::from("x"), Value::from(-3i64)),
        (Value::from("y"), Value::from(4i64)),
    ]);
    match call("manhattan", 3, vec![point]) {
        Message::Response(r) => assert_eq!(r.result.as_i64(), Some(7)),
        other => panic!("expected response, got {other:?}"),
    }
}

#[test]
fn unknown_function_is_an_error() {
    match call("nope", 4, vec![]) {
        Message::Error(e) => assert_eq!(e.id, 4),
        other => panic!("expected error, got {other:?}"),
    }
}

#[test]
fn arity_mismatch_is_an_error() {
    match call("add_numbers", 5, vec![Value::from(1i64)]) {
        Message::Error(e) => assert_eq!(e.id, 5),
        other => panic!("expected error, got {other:?}"),
    }
}

#[test]
fn bad_argument_type_is_an_error() {
    match call("add_numbers", 6, vec![Value::from("notnum"), Value::from(2i64)]) {
        Message::Error(e) => assert_eq!(e.id, 6),
        other => panic!("expected error, got {other:?}"),
    }
}
