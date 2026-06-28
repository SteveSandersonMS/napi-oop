//! The `#[napi]` attribute macro for `napi-oop`.
//!
//! The same annotated source builds two ways, selected by cargo feature:
//!
//! - **`in-proc`** (default): behave like a normal in-process napi-rs build.
//!   Phase 3 will delegate to napi-rs's real `#[napi]`.
//! - **`out-of-proc`**: emit out-of-process remoting glue — a wire-codec
//!   dispatch thunk plus a registry entry the runtime advertises to Node.
//!   Phase 3 implements the codegen.
//!
//! For now (Phase 1 scaffolding) both modes pass the item through unchanged;
//! only the build-mode *plumbing* is in place.

use proc_macro::TokenStream;

/// Drop-in replacement for napi-rs's `#[napi]`. See the crate docs for the
/// in-proc vs out-of-proc build modes.
#[proc_macro_attribute]
pub fn napi(_attr: TokenStream, item: TokenStream) -> TokenStream {
    #[cfg(feature = "out-of-proc")]
    {
        out_of_proc::expand(_attr, item)
    }
    #[cfg(not(feature = "out-of-proc"))]
    {
        in_proc::expand(_attr, item)
    }
}

#[cfg(not(feature = "out-of-proc"))]
mod in_proc {
    use proc_macro::TokenStream;

    /// In-process mode. Phase 3: delegate to napi-rs's `#[napi]`. For now,
    /// pass the annotated item through unchanged.
    pub(super) fn expand(_attr: TokenStream, item: TokenStream) -> TokenStream {
        item
    }
}

#[cfg(feature = "out-of-proc")]
mod out_of_proc {
    use proc_macro::TokenStream;

    /// Out-of-process mode. Phase 3: emit the wire-codec dispatch thunk and a
    /// `napi-oop` registry entry. For now, pass the item through unchanged.
    pub(super) fn expand(_attr: TokenStream, item: TokenStream) -> TokenStream {
        item
    }
}
