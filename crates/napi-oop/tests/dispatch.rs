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

/// Async fns dispatch through the generated `block_on`, returning a plain value.
#[napi]
pub async fn slow_double(n: i32) -> i32 {
    n * 2
}

/// Panics on purpose: the dispatcher must catch the unwind and reply with an
/// error rather than letting the worker thread die and hang the caller.
#[napi]
pub fn boom(_n: i32) -> i32 {
    panic!("kaboom");
}

/// A callback param: the macro decodes a handle marker and builds a closure that
/// fires through the `Callbacks` table. Sums values, notifying each step.
#[napi]
pub fn sum_each(values: Vec<i32>, on_step: impl Fn(i32)) -> i32 {
    let mut total = 0;
    for v in values {
        total += v;
        on_step(total);
    }
    total
}

/// The explicit form: a `ThreadsafeFunction<T>` stored and fired via `.call`.
#[napi]
pub fn sum_each_tsfn(values: Vec<i32>, on_step: napi_oop::ThreadsafeFunction<i32>) -> i32 {
    use napi_oop::ThreadsafeFunctionCallMode::NonBlocking;
    let mut total = 0;
    for v in values {
        total += v;
        on_step.call(total, NonBlocking);
    }
    total
}

fn call(function: &str, id: u64, args: Vec<Value>) -> Message {
    let cb: std::sync::Arc<dyn registry::Callbacks> = std::sync::Arc::new(registry::NoCallbacks);
    registry::dispatch(Request {
        id,
        function: function.to_string(),
        args,
    }, &cb)
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

#[test]
fn panic_in_function_is_an_error_not_a_hang() {
    match call("boom", 8, vec![Value::from(1i64)]) {
        Message::Error(e) => {
            assert_eq!(e.id, 8);
            assert!(e.message.contains("panicked"), "got: {}", e.message);
        }
        other => panic!("expected error, got {other:?}"),
    }
}

#[test]
fn dispatches_async_function_via_block_on() {
    match call("slow_double", 7, vec![Value::from(21i64)]) {
        Message::Response(r) => assert_eq!(r.result.as_i64(), Some(42)),
        other => panic!("expected response, got {other:?}"),
    }
}

#[test]
fn manifest_flags_async_from_keyword_not_return_type() {
    let m = napi_oop::manifest::manifest();
    let f = m.functions.iter().find(|f| f.rust_name == "slow_double").unwrap();
    // Return type is the unwrapped `i32` -> `number`, but the fn is marked async
    // purely from the `async` keyword on its signature.
    assert!(f.is_async);
    assert_eq!(f.ret, "number");
    let sync = m.functions.iter().find(|f| f.rust_name == "add_numbers").unwrap();
    assert!(!sync.is_async);
}

/// Records every callback invocation, plus any released handles.
struct RecordingCallbacks {
    steps: std::sync::Mutex<Vec<i64>>,
    released: std::sync::Mutex<Vec<u64>>,
}

impl registry::Callbacks for RecordingCallbacks {
    fn invoke(&self, _handle: u64, args: Vec<Value>) {
        self.steps.lock().unwrap().push(args[0].as_i64().unwrap());
    }
    fn release(&self, handle: u64) {
        self.released.lock().unwrap().push(handle);
    }
}

fn record(function: &str) -> (Message, std::sync::Arc<RecordingCallbacks>) {
    let cb = std::sync::Arc::new(RecordingCallbacks {
        steps: std::sync::Mutex::new(Vec::new()),
        released: std::sync::Mutex::new(Vec::new()),
    });
    let dyn_cb: std::sync::Arc<dyn registry::Callbacks> = cb.clone();
    let values = Value::Array(vec![Value::from(10i64), Value::from(20i64), Value::from(30i64)]);
    let handle = Value::Map(vec![(Value::from("__napi_cb"), Value::from(7u64))]);
    let reply = registry::dispatch(
        Request { id: 1, function: function.into(), args: vec![values, handle] },
        &dyn_cb,
    );
    (reply, cb)
}

#[test]
fn callback_impl_fn_invokes_through_callbacks_table() {
    let (reply, cb) = record("sum_each");
    match reply {
        Message::Response(r) => assert_eq!(r.result.as_i64(), Some(60)),
        other => panic!("expected response, got {other:?}"),
    }
    assert_eq!(*cb.steps.lock().unwrap(), vec![10, 30, 60]);
}

#[test]
fn threadsafe_function_invokes_through_callbacks_table() {
    let (reply, cb) = record("sum_each_tsfn");
    match reply {
        Message::Response(r) => assert_eq!(r.result.as_i64(), Some(60)),
        other => panic!("expected response, got {other:?}"),
    }
    assert_eq!(*cb.steps.lock().unwrap(), vec![10, 30, 60]);
}

#[test]
fn callback_handle_is_released_when_closure_drops() {
    // Both forms must release handle 7 once the Rust callback drops at call end.
    for name in ["sum_each", "sum_each_tsfn"] {
        let (_reply, cb) = record(name);
        assert_eq!(*cb.released.lock().unwrap(), vec![7], "{name} should release");
    }
}

#[test]
fn callback_manifest_renders_ts_fn_type() {
    let m = napi_oop::manifest::manifest();
    for name in ["sum_each", "sum_each_tsfn"] {
        let f = m.functions.iter().find(|f| f.rust_name == name).unwrap();
        assert_eq!(f.params, vec!["Array<number>", "(a0:number)=>void"], "{name}");
    }
}
