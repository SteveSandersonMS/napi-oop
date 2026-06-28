//! The `napi` facade: a drop-in for the napi-rs crate path so existing
//! `#[napi]` source compiles unchanged, two ways:
//! - **in-proc** (default): re-export the real napi-rs crates — produces a
//!   native `.node` addon.
//! - **out-of-proc**: re-export napi-oop shims — runs in a separate process.
//! The path stays `napi::…`; only the runtime differs.

#[cfg(feature = "out-of-proc")]
pub use napi_oop_macro::napi;
#[cfg(feature = "out-of-proc")]
pub use napi_oop::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
#[cfg(feature = "out-of-proc")]
pub mod threadsafe_function {
    pub use napi_oop::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
}
#[cfg(feature = "out-of-proc")]
pub mod bindgen_prelude {
    pub use napi_oop::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
}

// In-proc: pass real napi-rs through under the `napi::` path.
#[cfg(not(feature = "out-of-proc"))]
pub use ::napi::*;
#[cfg(not(feature = "out-of-proc"))]
pub use napi_derive::napi;
