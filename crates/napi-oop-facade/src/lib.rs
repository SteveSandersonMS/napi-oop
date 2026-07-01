//! The `napi` facade: a drop-in for the napi-rs crate path so existing `#[napi]`
//! source compiles unchanged and builds a **dual-ABI** cdylib from one source:
//! - loaded by Node directly, it is a normal in-process napi addon (real napi-rs
//!   ABI);
//! - loaded by a thin Rust provider exe, the same cdylib serves a Node child
//!   out-of-process via napi-oop.
//!
//! The path stays `napi::…`. We re-export the **real** napi-rs surface (so the
//! code napi-derive generates — `napi::bindgen_prelude::*`, `napi::sys`,
//! `napi::Result`, …— resolves) and then override the handful of value types
//! (`Buffer`, `BigInt`, `External`, `Utf16String`, `ThreadsafeFunction`) with
//! napi-oop's unified equivalents. Those unified types implement BOTH the real
//! napi traits (for the in-proc ABI) and serde (for the out-of-proc wire), so
//! one compiled function works on both paths. Explicit named re-exports shadow
//! the glob, so the unified types win over the real napi ones.

pub use ::real_napi::*;

// Unified value types override the real napi ones (explicit > glob).
pub use napi_oop::{
    BigInt, Buffer, External, Promise, ThreadsafeFunction, ThreadsafeFunctionCallMode, Utf16String,
};

// The dual-emit attribute macro: emits the real `#[napi]` ABI alongside the
// out-of-process wire glue.
pub use napi_oop_macro::napi;

pub mod threadsafe_function {
    pub use napi_oop::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
}

pub mod bindgen_prelude {
    pub use ::real_napi::bindgen_prelude::*;
    // Override the value types here too, since napi-derive emits fully-qualified
    // `napi::bindgen_prelude::Buffer` etc. in its generated code.
    pub use napi_oop::{
        BigInt, Buffer, External, Promise, ThreadsafeFunction, ThreadsafeFunctionCallMode,
        Utf16String,
    };
}
