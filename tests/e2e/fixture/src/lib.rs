//! E2E test fixture: a set of `#[napi]` functions exercising every cross-process
//! flow napi-oop supports — sync, async, both callback forms, Buffer, BigInt,
//! and External (incl. a slab probe to prove GC release).
//!
//! Built as a single **dual-ABI cdylib**. Node loads it directly as an in-process
//! napi addon (the real napi ABI emitted by the macro); a thin Rust host exe
//! dlopens it and calls [`napi_oop_e2e_main`] to serve a Node child out-of-process
//! over napi-oop. Both doors are generated from the same `#[napi]` source.

use std::process::Command;

use napi::napi;
use napi_oop::bootstrap::SOCKET_ENV;
use napi_oop::provider::{serve_from_env, spawn_and_serve};

#[napi]
pub fn add_numbers(a: i32, b: i32) -> i32 {
    a + b
}

#[napi]
pub async fn multiply_slow(a: i32, b: i32) -> i32 {
    std::thread::sleep(std::time::Duration::from_millis(200));
    a * b
}

/// An `Option<String>` parameter: a missing/`undefined` argument must arrive as
/// `None` (encoded over the wire as MessagePack nil, not msgpackr's `undefined`
/// extension), and a present value as `Some`.
#[napi]
pub fn greet(name: Option<String>) -> String {
    format!("hello, {}", name.as_deref().unwrap_or("world"))
}

/// A required param followed by a trailing `Option<T>`. napi-rs lets a caller
/// omit the trailing optional entirely, so the binding may send *fewer* args
/// than the declared arity; the missing tail must decode provider-side as
/// `None` (rather than being rejected for arity).
#[napi]
pub fn scale(value: i32, factor: Option<i32>) -> i32 {
    value * factor.unwrap_or(1)
}

#[napi]
pub fn sum_each(values: Vec<i32>, on_step: impl Fn(i32)) -> i32 {
    let mut total = 0;
    for v in values {
        total += v;
        on_step(total);
    }
    total
}

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

/// A callback the provider stores past the call, like a server's long-lived
/// accept callback. While the provider holds it, the caller's process must stay
/// alive — the live callback keeps the event loop ref'd, mirroring how an
/// in-process `ThreadsafeFunction` is ref'd by default until dropped.
static HELD_CALLBACK: std::sync::Mutex<Option<napi::ThreadsafeFunction<i32>>> =
    std::sync::Mutex::new(None);

/// Store the callback provider-side so it outlives the call.
#[napi]
pub fn hold_callback(cb: napi::ThreadsafeFunction<i32>) {
    *HELD_CALLBACK.lock().unwrap() = Some(cb);
}

/// Drop the held callback. Its `Drop` sends a `release` over the wire, letting
/// the caller's event loop drain so the process can exit on its own.
#[napi]
pub fn release_callback() {
    *HELD_CALLBACK.lock().unwrap() = None;
}

/// Abruptly terminate the provider mid-dispatch, simulating a crash or a signal
/// killing it (e.g. Ctrl+C reaching the child). The caller's in-flight call must
/// reject rather than block forever, and any callback the provider was holding
/// must stop keeping the caller's event loop alive.
#[napi]
pub fn exit_provider() {
    std::process::exit(0);
}

#[napi]
pub fn reverse_bytes(b: napi::Buffer) -> napi::Buffer {
    let mut v = b.to_vec();
    v.reverse();
    napi::Buffer::from(v)
}

#[napi]
pub fn double_big(n: napi::BigInt) -> napi::BigInt {
    // Exercise both `get_u64()` and the `words: Vec<u64>` struct-literal ctor,
    // matching how handle-token APIs build a BigInt.
    let (_sign, value, _lossless) = n.get_u64();
    napi::BigInt {
        sign_bit: false,
        words: vec![value.wrapping_mul(2)],
    }
}

/// Echo a BigInt unchanged, preserving sign and every 64-bit word. Proves the
/// wire carries arbitrary-precision BigInts (wider than 64 bits, and negative)
/// identically to the in-proc napi door, rather than truncating to one word.
#[napi]
pub fn echo_big(n: napi::BigInt) -> napi::BigInt {
    napi::BigInt {
        sign_bit: n.sign_bit,
        words: n.words,
    }
}

/// A free function declared with an explicit `#[napi(js_name = "…")]`. The JS
/// name (`bertShout`) is deliberately *not* the camelCase of the Rust name
/// (`shout`), so the binding must dispatch by the manifest's `rust_name` rather
/// than `camelToSnake(jsName)` — the regression this fixture guards.
#[napi(js_name = "bertShout")]
pub fn shout(text: String) -> String {
    text.to_uppercase()
}

/// A `#[napi(object)]` value struct: a plain by-value record crossing the
/// boundary by serde. The snake_case field proves camelCase exposure on TS.
#[napi(object)]
pub struct Point {
    pub x: i32,
    pub y: i32,
    pub label_text: String,
}

/// Returns an object by value (round-trips as a MessagePack map, typed by the
/// generated `Point` interface).
#[napi]
pub fn make_point(x: i32, y: i32, label_text: String) -> Point {
    Point { x, y, label_text }
}

/// Takes an object by value and reads its fields, proving the inbound decode.
#[napi]
pub fn describe_point(p: Point) -> String {
    format!("{}=({},{})", p.label_text, p.x, p.y)
}

/// A nested `#[napi(object)]` returned inside an `Option` field of an outer
/// object, alongside a sibling `Option<object>` left `None`. This mirrors the
/// "prepared input or an error result" shape real tools use: exactly one of the
/// two optionals is `Some`. The inner object carries a required `f64` whose value
/// is integral (`1.0`) — the case where a whole-number float must still decode as
/// a truthy nested object (not collapse to nil), so the caller sees `.input`.
#[napi(object)]
pub struct PreparedShellInput {
    pub shell_id: String,
    pub delay: f64,
}

/// The error variant's payload: another `#[napi(object)]` carrying a field with a
/// `#[napi(ts_type = …)]` override (a string-literal union). This guards that a
/// `ts_type` field attribute on a *nested* object doesn't disturb the wire encode
/// of the enclosing result — even when the field itself is left `None`.
#[napi(object)]
pub struct ShellExecutionResult {
    pub text_result_for_llm: String,
    #[napi(ts_type = "'success' | 'failure' | 'timeout' | 'rejected' | 'denied'")]
    pub result_kind: String,
    pub session_log: Option<String>,
}

#[napi(object)]
pub struct ShellPrepareResult {
    pub input: Option<PreparedShellInput>,
    pub error_result: Option<ShellExecutionResult>,
}

/// Returns the success variant: `input` is `Some(nested object)` and
/// `error_result` is `None`. Takes the tool's raw JSON string arg like the real
/// prepare fns; the caller must observe a truthy `.input` with its fields intact
/// and a nil `.errorResult`.
#[napi]
pub fn prepare_shell(input_json: String) -> ShellPrepareResult {
    let delay = if input_json.contains("\"delay\"") { 1.0 } else { 0.0 };
    ShellPrepareResult {
        input: Some(PreparedShellInput {
            shell_id: "e2e-shell".into(),
            delay,
        }),
        error_result: None,
    }
}

/// A plain (non-`#[napi]`) payload held behind an `External` handle. Its fields
/// are read provider-side via `Deref`, never serialized to JS.
pub struct Image {
    width: i32,
    height: i32,
}

impl Image {
    fn area(&self) -> i32 {
        self.width * self.height
    }
}

/// Mint an `External<Image>` handle (the value stays provider-side).
#[napi]
pub fn image_make(width: i32, height: i32) -> napi::External<Image> {
    napi::External::new(Image { width, height })
}

/// Take `&External<Image>` and reach the inner value through `Deref`, exercising
/// the borrow-by-handle path (the image fns in real workloads work this way).
#[napi]
pub fn image_area(img: &napi::External<Image>) -> i32 {
    img.area()
}

#[napi]
pub fn make_counter(start: i32) -> napi::External<i32> {
    napi::External::new(start)
}

#[napi]
pub fn read_counter(handle: napi::External<i32>) -> i32 {
    handle.cloned().unwrap_or(-1)
}

/// Live `External` handles the provider still holds, so the suite can assert
/// GC-collected handles release their slab entry.
#[napi]
pub fn live_counters() -> i32 {
    napi_oop::types::external_slab_len() as i32
}

/// A stateful #[napi] class: ctor + mutating method + getter + a method that
/// returns a fresh instance, exercising class state living provider-side and
/// round-tripping by handle.
#[napi]
#[derive(Clone)]
pub struct Counter {
    value: i32,
}

/// A free-fn factory returning a class instance, exercising minting via the
/// generated Serialize impl (the cross-class/factory path).
#[napi]
pub fn make_counter_class(start: i32) -> Counter {
    Counter { value: start }
}

#[napi]
impl Counter {
    #[napi(constructor)]
    pub fn new(start: i32) -> Self {
        Counter { value: start }
    }

    #[napi]
    pub fn add(&mut self, n: i32) -> i32 {
        self.value += n;
        self.value
    }

    #[napi(getter)]
    pub fn value(&self) -> i32 {
        self.value
    }

    /// A method declared with `#[napi(js_name = "…")]`: the JS method name
    /// (`bertReset`) diverges from the Rust name (`reset`), so the class proxy
    /// must surface `bertReset` and dispatch by the `Counter.reset` wire name.
    #[napi(js_name = "bertReset")]
    pub fn reset(&mut self) -> i32 {
        self.value = 0;
        self.value
    }

    #[napi]
    pub fn fork(&self) -> Counter {
        Counter { value: self.value }
    }

    /// An async method returning a fresh instance, exercising async cross-method
    /// class returns over the async binding.
    #[napi]
    pub async fn fork_slow(&self, by: i32) -> Counter {
        std::thread::sleep(std::time::Duration::from_millis(50));
        Counter {
            value: self.value + by,
        }
    }

    /// A cross-class return: a method on one class returning a *different*
    /// (non-`Clone`, non-`Serialize`) class. The return-encoder mints it by move.
    #[napi]
    pub fn snapshot(&self) -> Tally {
        Tally { total: self.value }
    }

    /// An async mutating method, returning the new value as a Promise. napi-rs
    /// requires `&mut self` async methods to be `unsafe` (the receiver is held
    /// across an await); the keyword is the only change, semantics are unchanged.
    #[napi]
    pub async unsafe fn add_slow(&mut self, n: i32) -> i32 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        self.value += n;
        self.value
    }
}

/// A second `#[napi]` class that deliberately derives neither `Clone` nor
/// `Serialize`: returning it exercises the encoder's by-move mint (it cannot copy
/// or field-serialize the instance), covering free-fn and cross-class returns of
/// a type with no extra trait support.
#[napi]
pub struct Tally {
    total: i32,
}

#[napi]
impl Tally {
    #[napi(getter)]
    pub fn total(&self) -> i32 {
        self.total
    }
}

/// A free-fn factory returning a non-`Clone` class instance.
#[napi]
pub fn make_tally(n: i32) -> Tally {
    Tally { total: n }
}

/// A class whose JS-facing name differs from its Rust type name. Dispatch still
/// uses `BertBox.*` wire names, while the manifest/codegen surface `RenamedBox`.
#[napi(js_name = "RenamedBox")]
pub struct BertBox {
    value: i32,
}

#[napi]
impl BertBox {
    #[napi(constructor)]
    pub fn new(value: i32) -> Self {
        Self { value }
    }

    #[napi]
    pub fn bump(&mut self, by: i32) -> i32 {
        self.value += by;
        self.value
    }

    #[napi(getter)]
    pub fn value(&self) -> i32 {
        self.value
    }

    #[napi]
    pub fn duplicate(&self) -> Self {
        Self { value: self.value }
    }
}

/// A free-fn factory returning the renamed class, exercising return type mapping
/// and factory wrapping under the JS-facing class name.
#[napi]
pub fn make_bert_box(value: i32) -> BertBox {
    BertBox { value }
}

/// Out-of-process provider entry, exported for a thin host exe to dlopen and
/// call. It runs in the host's own process, so it reads `argv`/env directly —
/// serving an existing socket (`SOCKET_ENV` set, the Node-parent case), spawning
/// and serving a child command from argv (the Rust-parent case), or emitting the
/// manifest. Returns the process exit code for the host to propagate.
///
/// This is the out-of-process door of the dual-ABI cdylib; Node's in-process
/// `require()` uses the napi addon door (`napi_register_module_v1`) instead and
/// never calls this.
#[no_mangle]
pub extern "C" fn napi_oop_e2e_main() -> i32 {
    let mut argv = std::env::args().skip(1);
    let first = argv.next();
    if first.as_deref() == Some("--emit-manifest") {
        println!("{}", napi_oop::manifest::manifest_json());
        return 0;
    }
    let result = if std::env::var_os(SOCKET_ENV).is_some() {
        serve_from_env()
    } else {
        let mut child: Vec<String> = first.into_iter().collect();
        child.extend(argv);
        if child.is_empty() {
            eprintln!("usage: e2e-provider <child-command...> (or --emit-manifest)");
            return 2;
        }
        let mut command = Command::new(&child[0]);
        command.args(&child[1..]);
        spawn_and_serve(command)
    };
    if let Err(e) = result {
        eprintln!("[e2e-provider] error: {e}");
        return 1;
    }
    0
}
