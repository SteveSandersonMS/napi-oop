//! The function registry exposed to the Node peer, plus the request dispatcher.
//!
//! The out-of-proc `#[napi]` macro emits, for each annotated function, an
//! [`inventory::submit!`] of a [`RegisteredFn`]: its name plus a type-erased
//! dispatch thunk that decodes the wire args, calls the function, and encodes
//! the result. Registration happens automatically at startup — no manual wiring.
//!
//! [`registered_names`] feeds the `Hello` handshake; [`dispatch`] routes an
//! incoming [`Request`] to the matching thunk and produces the reply [`Message`].

use std::sync::Arc;

use rmpv::Value;

use crate::codec::{ErrorMsg, HandleId, Message, Request, Response};

/// Lets a dispatched function invoke a callback held by the peer (e.g. a JS
/// function passed as an argument). Modelled on napi's `ThreadsafeFunction`:
/// invocation is **fire-and-forget** — the call is queued to the peer's event
/// loop and returns immediately; there is no result back to Rust.
///
/// Must be `Send + Sync` so a [`crate::ThreadsafeFunction`] can outlive the call
/// and fire from any thread.
pub trait Callbacks: Send + Sync {
    /// Queue an invocation of the peer-held callback `handle` with `args`.
    /// Returns once enqueued; runs asynchronously on the peer.
    fn invoke(&self, handle: HandleId, args: Vec<Value>);

    /// Tell the peer it may drop `handle` — sent when the Rust side stops
    /// holding the callback (closure dropped, or last `ThreadsafeFunction` gone).
    fn release(&self, handle: HandleId);
}

/// A type-erased dispatch thunk: decodes args, calls the function, encodes the
/// result. The shared [`Callbacks`] handle lets the function reach peer
/// callbacks — and lets a stored `ThreadsafeFunction` keep firing afterwards.
/// Returns `Err(message)` if decoding or the call itself fails.
pub type DispatchFn = fn(Vec<Value>, &Arc<dyn Callbacks>) -> Result<Value, String>;

/// One registered `#[napi]` function, collected via [`inventory`].
pub struct RegisteredFn {
    /// The exported function name advertised to the peer. This is the wire name
    /// the dispatcher routes on — the Rust function name (free fns) or
    /// `Class.method` (methods) — and is independent of the JS-facing name.
    pub name: &'static str,
    /// Explicit JS-facing name from `#[napi(js_name = "…")]`, or `""` if none was
    /// given (in which case the manifest derives the camelCase form of `name`).
    /// Dispatch never uses this; it only steers the surfaced TS name so the
    /// out-of-proc surface matches what napi-rs would expose in-proc.
    pub js_name: &'static str,
    /// The thunk that services a call to this function.
    pub dispatch: DispatchFn,
    /// The Rust type of each parameter, in declaration order (e.g. `["i64","i64"]`).
    /// The IDL the TS generator maps to TS types.
    pub params: &'static [&'static str],
    /// The declared name of each parameter, in order (e.g. `["a","b"]`).
    pub param_names: &'static [&'static str],
    /// The Rust return type (e.g. `"i64"`). For an `async fn`, this is the
    /// unwrapped `Output` type; the generator wraps it in `Promise<>`.
    pub ret: &'static str,
    /// Whether the function is `async`. Async fns surface as `Promise<T>` on TS
    /// in *both* the async and sync bindings (sync mode must not hide async).
    pub is_async: bool,
}

inventory::collect!(RegisteredFn);

/// One method of a `#[napi]` class. The dispatch thunk is registered as a normal
/// [`RegisteredFn`] under the wire name `Class.method`; this entry carries the
/// extra metadata the TS generator needs to emit a class proxy.
pub struct RegisteredMethod {
    /// Owning class name (`SandboxHandle`).
    pub class: &'static str,
    /// JS method name (`isAlive`); `"constructor"` for the constructor.
    pub method: &'static str,
    /// Wire name the dispatcher routes on (`SandboxHandle.is_alive`).
    pub rust_name: &'static str,
    pub params: &'static [&'static str],
    pub param_names: &'static [&'static str],
    pub ret: &'static str,
    pub is_async: bool,
    /// True for a `#[napi(getter)]` — emitted as a TS accessor, not a method.
    pub is_getter: bool,
}

inventory::collect!(RegisteredMethod);

/// A JS-facing class rename declared on the class struct with
/// `#[napi(js_name = "…")]`. Method dispatch remains keyed by the Rust class
/// name; this only affects manifest/type names surfaced to TypeScript.
pub struct RegisteredClassRename {
    pub rust_name: &'static str,
    pub js_name: &'static str,
}

inventory::collect!(RegisteredClassRename);

/// One `#[napi(object)]` struct: a plain value type that crosses the boundary by
/// serde (camelCase fields, matching napi-rs). Carries the field shape so the TS
/// generator can emit a matching `interface`, rather than falling back to
/// `unknown`. Field types are the Rust type strings, mapped to TS by the manifest.
pub struct RegisteredObject {
    /// Struct name, used verbatim as the TS interface name.
    pub name: &'static str,
    /// Field names in declaration order (snake_case; the generator camelCases them).
    pub field_names: &'static [&'static str],
    /// Field Rust types, in order, aligned with `field_names`.
    pub field_types: &'static [&'static str],
}

inventory::collect!(RegisteredObject);

/// One `#[napi]` constant: a compile-time value exported to JS. In-process,
/// napi-rs exposes it as a JS `const`; out-of-process there is nothing to
/// *call*, so the value is baked into the manifest and the peer reads it
/// directly (no dispatch round-trip). Mirrors napi-rs, which exports the const
/// under its Rust name verbatim (e.g. SCREAMING_SNAKE), not camelCased.
pub struct RegisteredConst {
    /// Rust const name, exported to JS verbatim (as napi-rs does).
    pub name: &'static str,
    /// Explicit JS-facing name from `#[napi(js_name = "…")]`, or `""` to use
    /// [`name`](Self::name) verbatim.
    pub js_name: &'static str,
    /// The Rust type of the constant (e.g. `"i64"`), mapped to TS by the
    /// manifest so the generated binding types the value correctly.
    pub rust_type: &'static str,
    /// Thunk producing the constant's JSON value. Evaluated once at manifest
    /// emit time and embedded verbatim, so the peer never dispatches for it.
    pub value: fn() -> serde_json::Value,
}

inventory::collect!(RegisteredConst);

/// Serialize a `#[napi]` constant's value to JSON for embedding in the manifest.
/// Used by the `#[napi]` macro's generated [`RegisteredConst`] thunk; falls back
/// to `null` for the (practically impossible) case of a non-serializable const.
pub fn const_value_json<T: serde::Serialize>(value: &T) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
}

/// A [`Callbacks`] that drops every invocation — for fns that take no callbacks
/// and for tests. (Fire-and-forget, so dropping is observably "queued, ignored".)
pub struct NoCallbacks;

impl Callbacks for NoCallbacks {
    fn invoke(&self, _handle: HandleId, _args: Vec<Value>) {}
    fn release(&self, _handle: HandleId) {}
}

/// Look up a registered function by exported name.
pub fn lookup(name: &str) -> Option<&'static RegisteredFn> {
    inventory::iter::<RegisteredFn>
        .into_iter()
        .find(|f| f.name == name)
}

/// The names of all registered functions, for the `Hello` handshake.
pub fn registered_names() -> Vec<String> {
    inventory::iter::<RegisteredFn>
        .into_iter()
        .map(|f| f.name.to_string())
        .collect()
}

/// Route a [`Request`] to its registered function, producing the reply message
/// (a [`Message::Response`] on success or [`Message::Error`] on failure). The
/// `callbacks` handle lets the function invoke any JS callbacks passed as args.
pub fn dispatch(request: Request, callbacks: &Arc<dyn Callbacks>) -> Message {
    let Request { id, function, args } = request;
    if crate::diag::enabled() {
        crate::diag::log(&format!(
            "\"event\":\"dispatch-start\",\"id\":{id},\"fn\":{function:?},\"nargs\":{}",
            args.len()
        ));
    }
    let reply = match lookup(&function) {
        Some(registered) => {
            // Guard against a function panicking: an unwind would otherwise kill
            // the worker thread without ever replying, leaving the caller hung.
            let before = crate::types::external_mint_count();
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _cb_scope = crate::tsfn::push_callbacks(Arc::clone(callbacks));
                (registered.dispatch)(args, callbacks)
            }));
            let minted = crate::types::external_mint_count().saturating_sub(before);
            match outcome {
                Ok(Ok(result)) => {
                    // Any External minted by this call must surface top-level, where
                    // the TS finalizer can wrap it and drive release. A token nested
                    // inside the result is unreachable for cleanup, so reject loudly
                    // rather than leak it.
                    if minted > top_level_externals(&result) {
                        Message::Error(ErrorMsg {
                            id,
                            message: format!(
                                "function '{function}' returned an External nested below \
                                 top level, which cannot be released; return it directly"
                            ),
                        })
                    } else {
                        Message::Response(Response { id, result })
                    }
                }
                Ok(Err(message)) => Message::Error(ErrorMsg { id, message }),
                Err(panic) => Message::Error(ErrorMsg {
                    id,
                    message: format!("function '{function}' panicked: {}", panic_message(&panic)),
                }),
            }
        }
        None => Message::Error(ErrorMsg {
            id,
            message: format!("unknown function: {function}"),
        }),
    };
    if crate::diag::enabled() {
        let (kind, detail) = match &reply {
            Message::Response(r) => ("response", crate::diag::describe_value(&r.result)),
            Message::Error(e) => ("error", e.message.clone()),
            _ => ("other", String::new()),
        };
        crate::diag::log(&format!(
            "\"event\":\"dispatch-done\",\"id\":{id},\"fn\":{function:?},\"kind\":\"{kind}\",\"detail\":{detail:?}"
        ));
    }
    reply
}

/// Best-effort extraction of a panic's message payload.
fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".to_string()
    }
}

/// Count External markers reachable at the top level — the value itself, or the
/// direct elements of a top-level array. Externals deeper than this can't be
/// wrapped by the TS finalizer, so the dispatcher rejects calls that mint more.
fn top_level_externals(v: &Value) -> u64 {
    let is_marker = v
        .as_map()
        .map(|m| m.len() == 1 && m[0].0.as_str() == Some(crate::types::EXTERNAL_KEY))
        .unwrap_or(false);
    if is_marker {
        1
    } else {
        0
    }
}
