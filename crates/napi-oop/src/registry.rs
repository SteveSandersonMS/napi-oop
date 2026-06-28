//! The function registry exposed to the Node peer, plus the request dispatcher.
//!
//! The out-of-proc `#[napi]` macro emits, for each annotated function, an
//! [`inventory::submit!`] of a [`RegisteredFn`]: its name plus a type-erased
//! dispatch thunk that decodes the wire args, calls the function, and encodes
//! the result. Registration happens automatically at startup — no manual wiring.
//!
//! [`registered_names`] feeds the `Hello` handshake; [`dispatch`] routes an
//! incoming [`Request`] to the matching thunk and produces the reply [`Message`].

use rmpv::Value;

use crate::codec::{ErrorMsg, Message, Request, Response};

/// A type-erased dispatch thunk: decodes args, calls the function, encodes the
/// result. Returns `Err(message)` if decoding or the call itself fails.
pub type DispatchFn = fn(Vec<Value>) -> Result<Value, String>;

/// One registered `#[napi]` function, collected via [`inventory`].
pub struct RegisteredFn {
    /// The exported function name advertised to the peer.
    pub name: &'static str,
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
/// (a [`Message::Response`] on success or [`Message::Error`] on failure).
pub fn dispatch(request: Request) -> Message {
    let Request { id, function, args } = request;
    match lookup(&function) {
        Some(registered) => match (registered.dispatch)(args) {
            Ok(result) => Message::Response(Response { id, result }),
            Err(message) => Message::Error(ErrorMsg { id, message }),
        },
        None => Message::Error(ErrorMsg {
            id,
            message: format!("unknown function: {function}"),
        }),
    }
}
