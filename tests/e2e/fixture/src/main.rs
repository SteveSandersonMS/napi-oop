//! E2E test fixture provider. A dedicated set of `#[napi]` functions exercising
//! every cross-process flow napi-oop supports — sync, async, both callback
//! forms, Buffer, BigInt, and External (incl. a slab probe to prove GC release).
//! Kept separate from the examples so the examples stay clean and human-facing.

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
        on_step.call(total, NonBlocking);
    }
    total
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
    napi::BigInt { sign_bit: false, words: vec![value.wrapping_mul(2)] }
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

    #[napi]
    pub fn fork(&self) -> Counter {
        Counter { value: self.value }
    }

    /// An async method returning a fresh instance, exercising async cross-method
    /// class returns over the async binding.
    #[napi]
    pub async fn fork_slow(&self, by: i32) -> Counter {
        std::thread::sleep(std::time::Duration::from_millis(50));
        Counter { value: self.value + by }
    }

    /// An async mutating method, returning the new value as a Promise.
    #[napi]
    pub async fn add_slow(&mut self, n: i32) -> i32 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        self.value += n;
        self.value
    }
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
            eprintln!("usage: e2e-provider <child-command...> (or --emit-manifest)");
            std::process::exit(2);
        }
        let mut command = Command::new(&child[0]);
        command.args(&child[1..]);
        spawn_and_serve(command)
    };
    if let Err(e) = result {
        eprintln!("[e2e-provider] error: {e}");
        std::process::exit(1);
    }
}
