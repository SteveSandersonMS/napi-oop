//! Env-gated wire diagnostics for the provider side.
//!
//! Mirrors the TypeScript `diag` facility in `napi-oop-runtime`: when
//! `NAPI_OOP_DIAG` names a file, each event is appended as one JSON line tagged
//! `"role":"provider"`. Pointing both sides at the same path yields a single
//! file that interleaves the whole wire — the caller's `main`/`worker` events
//! and the provider's `dispatch` events — ordered by timestamp, which is what
//! distinguishes a provider-side corruption from a caller-side mis-delivery.
//! When the env var is unset, logging is a cheap no-op.

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use rmpv::Value;

fn diag_file() -> Option<&'static str> {
    static PATH: OnceLock<Option<String>> = OnceLock::new();
    PATH.get_or_init(|| match std::env::var("NAPI_OOP_DIAG") {
        Ok(p) if !p.is_empty() => Some(p),
        _ => None,
    })
    .as_deref()
}

/// Whether provider diagnostics are enabled for this process.
pub fn enabled() -> bool {
    diag_file().is_some()
}

/// Append one diagnostic record. `fields` is the JSON body (comma-separated
/// `"key":value` pairs, no surrounding braces). No-op when disabled.
pub fn log(fields: &str) {
    let Some(path) = diag_file() else {
        return;
    };
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    // `Debug` of a `String` yields a valid, escaped JSON string literal.
    let tid = format!("{:?}", std::thread::current().id());
    let pid = std::process::id();
    let line = format!(
        "{{\"ts\":{ts},\"pid\":{pid},\"role\":\"provider\",\"thread\":{tid:?},{fields}}}\n"
    );
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// A compact one-line summary of a wire value, for a diagnostic `detail` field.
/// For a map it lists each key and the *kind* of its value, so the both-null
/// case (`map{input=nil,error_result=nil}`) is visible at a glance — the exact
/// signature of a result the caller would reject as "no input or result".
pub fn describe_value(v: &Value) -> String {
    match v {
        Value::Nil => "nil".into(),
        Value::Boolean(b) => format!("bool({b})"),
        Value::Integer(i) => format!("int({i})"),
        Value::F32(f) => format!("f32({f})"),
        Value::F64(f) => format!("f64({f})"),
        Value::String(s) => format!("str({:?})", s.as_str().unwrap_or("<non-utf8>")),
        Value::Binary(b) => format!("bin[{}]", b.len()),
        Value::Array(a) => format!("array[{}]", a.len()),
        Value::Map(m) => {
            let entries: Vec<String> = m
                .iter()
                .map(|(k, val)| format!("{}={}", k.as_str().unwrap_or("?"), value_kind(val)))
                .collect();
            format!("map{{{}}}", entries.join(","))
        }
        Value::Ext(t, d) => format!("ext({t},[{}])", d.len()),
    }
}

fn value_kind(v: &Value) -> &'static str {
    match v {
        Value::Nil => "nil",
        Value::Boolean(_) => "bool",
        Value::Integer(_) => "int",
        Value::F32(_) | Value::F64(_) => "num",
        Value::String(_) => "str",
        Value::Binary(_) => "bin",
        Value::Array(_) => "array",
        Value::Map(_) => "map",
        Value::Ext(_, _) => "ext",
    }
}
