//! Thin out-of-process host for the tokio-fetch example. It carries none of the
//! runtime itself: it locates the sibling dual-ABI cdylib (the SAME artifact Node
//! loads in-process), `dlopen`s it, and calls its exported
//! `tokio_fetch_provider_main` entry, propagating the returned exit code. The
//! provider logic — serve-an-existing-socket / spawn-and-serve-a-child /
//! emit-manifest — all lives in the cdylib and reads this process's argv/env.

use std::path::PathBuf;

/// Platform file name of the example cdylib, co-located with this exe.
fn lib_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "tokio_fetch_example.dll"
    } else if cfg!(target_os = "macos") {
        "libtokio_fetch_example.dylib"
    } else {
        "libtokio_fetch_example.so"
    }
}

fn lib_path() -> PathBuf {
    let exe = std::env::current_exe().expect("resolve current exe");
    exe.parent()
        .expect("exe has a parent directory")
        .join(lib_name())
}

fn main() {
    let lib_path = lib_path();
    let code = unsafe {
        let lib = libloading::Library::new(&lib_path)
            .unwrap_or_else(|e| panic!("failed to load example cdylib {lib_path:?}: {e}"));
        let entry: libloading::Symbol<extern "C" fn() -> i32> = lib
            .get(b"tokio_fetch_provider_main\0")
            .expect("example cdylib is missing the `tokio_fetch_provider_main` export");
        entry()
    };
    std::process::exit(code);
}
