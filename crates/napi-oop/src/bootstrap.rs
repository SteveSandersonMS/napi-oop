//! Process bootstrap shared by both peers: the socket-path env-var convention,
//! a cross-platform named-socket path generator, and cleanup.
//!
//! The **parent** generates a path with [`generate_socket_path`], binds a
//! listener there, then spawns the **child** with that path exported in the
//! [`SOCKET_ENV`] environment variable. The child reads the variable and
//! connects. This is symmetric: either Rust or Node may be the parent, and the
//! provider/caller roles are independent of who spawned whom.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Environment variable through which a parent passes the named-socket path to
/// the child it spawns. Mirrored by the Node runtime.
pub const SOCKET_ENV: &str = "NAPI_OOP_SOCKET";

/// Generate an unpredictable, platform-appropriate socket name.
///
/// The name is chosen so the transport leaves no stray artifact in a directory a
/// consumer might list:
/// * **Windows** — a named pipe (`\\.\pipe\…`); no filesystem entry.
/// * **Linux** — a bare name bound as an *abstract-namespace* socket, which has
///   no filesystem entry at all (see [`transport`](crate::transport)).
/// * **macOS/BSD** — a socket file, since those platforms have no abstract
///   namespace. It lives under the real OS temp dir (never a `TMPDIR`-overridden
///   one), so it can't land in the consumer's working directory.
pub fn generate_socket_path() -> String {
    let token = unique_token();
    let pid = std::process::id();
    if cfg!(windows) {
        format!(r"\\.\pipe\napi-oop-{pid}-{token}")
    } else if cfg!(any(target_os = "linux", target_os = "android")) {
        format!("napi-oop-{pid}-{token}")
    } else {
        real_os_temp_dir()
            .join(format!("napi-oop-{pid}-{token}.sock"))
            .to_string_lossy()
            .into_owned()
    }
}

/// The real OS temporary directory, deliberately ignoring the
/// `TMPDIR`/`TMP`/`TEMP` environment variables that [`std::env::temp_dir`]
/// honors. A consumer (or its test harness) may repoint those at a working
/// directory; the transport socket must never be created there, where it would
/// surface in the consumer's own directory listings.
fn real_os_temp_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        if let Some(dir) = darwin_user_temp_dir() {
            return dir;
        }
    }
    PathBuf::from("/tmp")
}

/// The macOS per-user temp dir (`/var/folders/…/T/`) from
/// `confstr(_CS_DARWIN_USER_TEMP_DIR)` — the OS-provisioned location, resolved
/// without consulting any environment variable.
#[cfg(target_os = "macos")]
fn darwin_user_temp_dir() -> Option<PathBuf> {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    // SAFETY: standard two-call `confstr` sizing; the buffer is sized to the
    // length `confstr` reports and only the written bytes are read back.
    unsafe {
        let needed = libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, std::ptr::null_mut(), 0);
        if needed == 0 {
            return None;
        }
        let mut buf = vec![0u8; needed];
        let written = libc::confstr(
            libc::_CS_DARWIN_USER_TEMP_DIR,
            buf.as_mut_ptr().cast(),
            needed,
        );
        if written == 0 || written > needed {
            return None;
        }
        buf.truncate(written - 1); // drop the trailing NUL
        Some(PathBuf::from(OsString::from_vec(buf)))
    }
}

/// Best-effort removal of the transport's socket file. Only macOS/BSD create
/// one; Linux (abstract namespace) and Windows (named pipe) have no filesystem
/// entry, so this is a no-op there.
pub fn cleanup_socket_path(path: &str) {
    #[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
    {
        let _ = std::fs::remove_file(path);
    }
    #[cfg(not(all(unix, not(any(target_os = "linux", target_os = "android")))))]
    {
        let _ = path;
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
