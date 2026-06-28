//! The duplex byte channel between the two peers.
//!
//! Phase 2 adds the concrete **named local socket** implementation (Unix domain
//! socket on Linux/macOS, named pipe on Windows). stdio is intentionally never
//! used, since the child process may need stdout/stderr for its own output.

use std::io::{Read, Write};

/// A bidirectional, ordered byte stream connecting the Node and Rust peers.
///
/// Any `Read + Write + Send` type qualifies, so tests can use in-memory pipes
/// and production can use the named-socket transport (Phase 2).
pub trait Transport: Read + Write + Send {}

impl<T: Read + Write + Send> Transport for T {}
