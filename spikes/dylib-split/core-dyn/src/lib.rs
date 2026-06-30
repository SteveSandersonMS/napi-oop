//! Thin dynamic re-export of the business logic. Exists only to create a shared
//! `dylib` boundary so `core`'s code is compiled once and shared by both
//! wrappers. Not built on musl (no Rust dylib support there).

pub use spike_core::*;
