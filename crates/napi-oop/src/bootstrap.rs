//! Process bootstrap shared by both peers: the socket-path env-var convention,
//! a cross-platform named-socket path generator, and cleanup.
//!
//! The **parent** generates a path with [`generate_socket_path`], binds a
//! listener there, then spawns the **child** with that path exported in the
//! [`SOCKET_ENV`] environment variable. The child reads the variable and
//! connects. This is symmetric: either Rust or Node may be the parent, and the
//! provider/caller roles are independent of who spawned whom.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Environment variable through which a parent passes the named-socket path to
/// the child it spawns. Mirrored by the Node runtime.
pub const SOCKET_ENV: &str = "NAPI_OOP_SOCKET";

/// Generate an unpredictable, platform-appropriate named-socket path: a named
/// pipe on Windows (`\\.\pipe\…`) or a socket file under the temp dir on Unix.
pub fn generate_socket_path() -> String {
    let token = unique_token();
    let pid = std::process::id();
    if cfg!(windows) {
        format!(r"\\.\pipe\napi-oop-{pid}-{token}")
    } else {
        std::env::temp_dir()
            .join(format!("napi-oop-{pid}-{token}.sock"))
            .to_string_lossy()
            .into_owned()
    }
}

/// Best-effort removal of a Unix socket file once a listener is done with it.
/// A no-op on Windows, where named pipes are reclaimed automatically.
pub fn cleanup_socket_path(path: &str) {
    if !cfg!(windows) {
        let _ = std::fs::remove_file(path);
    }
}

/// A process-unique hex token: nanosecond clock mixed with a monotonic counter,
/// so concurrent calls within the same process don't collide.
fn unique_token() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{nanos:x}-{counter:x}")
}
