//! The `napi` facade: a drop-in for the napi-rs crate path so existing
//! `#[napi]` source compiles unchanged. It re-exports the `#[napi]` attribute
//! macro and the user-facing types under the same names napi-rs uses, but backed
//! by napi-oop's out-of-process runtime.
//!
//! Source written for napi-rs (`use napi::ThreadsafeFunction`, `napi_derive`'s
//! `#[napi]`) builds two ways: in-proc against real napi-rs, or out-of-proc
//! against this. The path stays `napi::…`; only the runtime differs.

pub use napi_oop_macro::napi;

pub use napi_oop::{ThreadsafeFunction, ThreadsafeFunctionCallMode};

/// Mirror of napi-rs's `napi::threadsafe_function` module path so explicit
/// imports (`use napi::threadsafe_function::ThreadsafeFunction`) resolve too.
pub mod threadsafe_function {
    pub use napi_oop::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
}

/// Mirror of napi-rs's `bindgen_prelude`, the catch-all glob most code imports.
pub mod bindgen_prelude {
    pub use napi_oop::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
}
