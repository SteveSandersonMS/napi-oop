//! The peer: connection bootstrap, handshake, and the message router.
//!
//! Responsibilities (filled across phases):
//! - **Bootstrap** (Phase 5): the parent generates a unique socket path/pipe
//!   name, listens, and passes it to the spawned child via an env var; the
//!   child connects. Symmetric — either side may be the parent.
//! - **Handshake** (Phase 2): exchange `PROTOCOL_VERSION`, the function
//!   registry, and capabilities.
//! - **Router** (Phase 2/5): multiplex outstanding calls by correlation id; a
//!   call awaiting a reply must keep servicing inbound messages so re-entrant
//!   callbacks (JS→Rust→JS) don't deadlock.

// TODO(phase2): Peer { transport, pending, registry } with run-loop + handshake.
// TODO(phase5): parent/child bootstrap helpers over the named-socket transport.
