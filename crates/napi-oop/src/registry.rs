//! The function registry exposed to the Node peer.
//!
//! Phase 3's `#[napi]` macro emits, for each annotated function, a registration
//! entry collected at startup (via `inventory`/`linkme`) plus a type-erased
//! dispatch thunk that decodes wire args, calls the function, and encodes the
//! result. The `Hello` handshake advertises the registered names to the peer.

// TODO(phase3): RegisteredFn { name, dispatch } + inventory collection +
// a dispatcher that routes an incoming Request to the right thunk.
