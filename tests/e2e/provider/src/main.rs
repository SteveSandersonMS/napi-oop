//! Thin out-of-process host for the E2E fixture. It carries none of the runtime
//! itself: it locates the sibling dual-ABI cdylib (the SAME artifact Node loads
//! in-process), `dlopen`s it, and calls its exported `napi_oop_e2e_main` entry,
//! propagating the returned exit code. The provider logic — serve-an-existing-
//! socket / spawn-and-serve-a-child / emit-manifest — all lives in the cdylib and
//! reads this process's argv/env directly.

use std::path::PathBuf;

/// Platform file name of the fixture cdylib, co-located with this exe.
fn fixture_lib_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "napi_oop_e2e_fixture.dll"
    } else if cfg!(target_os = "macos") {
        "libnapi_oop_e2e_fixture.dylib"
    } else {
        "libnapi_oop_e2e_fixture.so"
    }
}

fn fixture_lib_path() -> PathBuf {
    let exe = std::env::current_exe().expect("resolve current exe");
    exe.parent()
        .expect("exe has a parent directory")
        .join(fixture_lib_name())
}

fn main() {
    let lib_path = fixture_lib_path();
    let code = unsafe {
        let lib = libloading::Library::new(&lib_path)
            .unwrap_or_else(|e| panic!("failed to load fixture cdylib {lib_path:?}: {e}"));
        let entry: libloading::Symbol<extern "C" fn() -> i32> = lib
            .get(b"napi_oop_e2e_main\0")
            .expect("fixture cdylib is missing the `napi_oop_e2e_main` export");
        entry()
    };
    std::process::exit(code);
}
