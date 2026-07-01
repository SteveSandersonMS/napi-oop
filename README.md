# napi-oop

`napi-oop` turns ordinary-looking napi-rs Rust source into a **single dual-ABI
`cdylib`** that can be hosted two ways from the same build output:

```
                     one Rust source
                 use napi::napi; #[napi]
                           │
                           ▼
                one built cdylib artifact
       ┌───────────────────┴───────────────────┐
       ▼                                       ▼
Node requires it as `.node`          Rust host dlopen()s the same cdylib
(real napi-rs / N-API ABI)           and calls <name>_provider_main()
       │                                       │
 synchronous native calls OK          local socket + MessagePack wire protocol
 no provider process                  no Node runtime inside the Rust host
```

The hosting mode is chosen by the **host**, not by a Cargo feature. Node can load
the artifact directly as a normal native addon, while an out-of-process host can
load the same file and enter napi-oop's provider loop through an exported C
symbol such as `add_numbers_provider_main`.

## Repository layout

```
crates/
  napi-oop/                 # Rust runtime: transport, registry, provider, manifest, wire types
  napi-oop-macro/           # dual-emit #[napi] attribute macro
  napi-oop-facade/          # compiled as crate name `napi`; keeps source on napi:: paths
packages/
  runtime/                  # npm package `napi-oop-runtime`: OOP transport + codegen
examples/
  add-numbers/              # dual-ABI cdylib plus a thin dlopen provider host
  tokio-fetch/              # async/tokio dual-ABI example plus provider host
tests/e2e/                  # proves in-proc, Node-parent OOP, and Rust-parent OOP
```

## Minimal Rust crate

`crates/napi-oop-facade` is compiled with `[lib] name = "napi"`. A consumer uses
it under the crate name `napi`, so source can keep writing `use napi::...`,
`napi::bindgen_prelude::*`, `napi::ThreadsafeFunction`, and `use napi::napi;`.
The facade re-exports the real napi-rs surface and overrides the value/callback
types that need to work on both ABIs (`Buffer`, `BigInt`, `External`,
`Utf16String`, `ThreadsafeFunction`).

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
napi-oop = { version = "0.1" }
napi = { package = "napi-oop-facade", version = "0.1" }

[build-dependencies]
napi-build = "2"
```

In this repository, examples use path dependencies instead:

```toml
napi-oop = { path = "../../crates/napi-oop" }
napi = { package = "napi-oop-facade", path = "../../crates/napi-oop-facade" }
```

If existing source imports the macro as `use napi_derive::napi;`, map that
package name to napi-oop's macro crate to avoid source churn:

```toml
napi-derive = { package = "napi-oop-macro", version = "0.1" }
```

The generated macro delegation is anchored at `::napi_oop::__derive::napi`, so
this remap does not recurse into itself; `napi-oop` re-exports the real
`napi_derive::napi` at that hidden path for the in-process ABI.

`build.rs` is the normal napi-rs setup:

```rust
fn main() {
    napi_build::setup();
}
```

A minimal dual-ABI library looks like this:

```rust
use std::process::Command;

use napi::napi;
use napi_oop::bootstrap::SOCKET_ENV;
use napi_oop::provider::{serve_from_env, spawn_and_serve};

#[napi]
pub fn add_numbers(a: i32, b: i32) -> i32 {
    a + b
}

#[napi]
pub async fn multiply_slow(a: i32, b: i32) -> i32 {
    std::thread::sleep(std::time::Duration::from_millis(200));
    a * b
}

#[napi]
pub struct Counter {
    value: i32,
}

#[napi]
impl Counter {
    #[napi(constructor)]
    pub fn new(value: i32) -> Self {
        Self { value }
    }

    #[napi]
    pub fn add(&mut self, n: i32) -> i32 {
        self.value += n;
        self.value
    }

    #[napi(getter)]
    pub fn value(&self) -> i32 {
        self.value
    }
}

#[no_mangle]
pub extern "C" fn my_provider_main() -> i32 {
    let mut argv = std::env::args().skip(1);
    let first = argv.next();

    if first.as_deref() == Some("--emit-manifest") {
        println!("{}", napi_oop::manifest::manifest_json());
        return 0;
    }

    let result = if std::env::var_os(SOCKET_ENV).is_some() {
        serve_from_env()
    } else {
        let mut child: Vec<String> = first.into_iter().collect();
        child.extend(argv);
        if child.is_empty() {
            eprintln!("usage: my-provider <child-command...> (or --emit-manifest)");
            return 2;
        }
        let mut command = Command::new(&child[0]);
        command.args(&child[1..]);
        spawn_and_serve(command)
    };

    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("provider error: {e}");
            1
        }
    }
}
```

Build the cdylib as usual:

```bash
cargo build --release -p your-crate
```

## How the dual ABI is emitted

`crates/napi-oop-macro` provides the `#[napi]` attribute. For each supported
annotated item it emits two doors:

1. the real napi-rs in-process adapter, by delegating to
   `::napi_oop::__derive::napi`; and
2. out-of-process manifest and dispatch registration used by the provider loop.

For value objects and classes, the macro also records enough shape information
for `napi-oop-runtime` to generate TypeScript bindings. There is no
`in-proc`/`out-of-proc` feature switch in the facade. Its features are passthrough
napi-rs features such as `async`, `serde-json`, `dyn-symbols`, and
`object-indexmap`.

## Why one artifact can load in both hosts

`napi-oop` enables napi-rs's `dyn-symbols` feature graph-wide. The `napi_*` C API
is resolved from the host process at runtime instead of being linked as a hard
build-time dependency. In-process, Node loads the addon, `napi_register_module_v1`
runs, and napi-rs binds symbols from the Node process. Out-of-process, the thin
Rust host never calls that registration path; it calls the exported provider C
symbol instead, so the N-API stubs are never invoked.

This is the cross-platform trick that lets the same Windows/macOS/Linux cdylib be
both a normal Node addon and a library that a non-Node Rust executable can
`dlopen`.

## Hosting mode 1: in-process Node addon

In this mode Node loads the cdylib as a `.node` native addon and all calls go
through real napi-rs/N-API. Sync functions are ordinary blocking native calls;
`async fn` exports are napi-rs promises; classes, `ThreadsafeFunction`, `Buffer`,
`BigInt`, `External`, and errors use the real napi-rs ABI.

The e2e suite stages the same fixture cdylib as `tests/e2e/fixture.node` and then
uses:

```js
const native = require('./fixture.node');
native.addNumbers(2, 3);          // 5
await native.multiplySlow(6, 7);  // Promise<number>
const c = new native.Counter(5);
```

For packaging, use the normal napi-rs approach: build a `cdylib` with
`napi-build`, copy or package the platform library with a `.node` filename, and
`require()` it from Node.

## Hosting mode 2: out-of-process provider

In OOP mode a small Rust executable loads the same cdylib with `libloading`,
resolves the exported provider entry point, and calls it:

```rust
let lib = unsafe { libloading::Library::new(lib_path)? };
let entry: libloading::Symbol<extern "C" fn() -> i32> =
    unsafe { lib.get(b"add_numbers_provider_main\0")? };
std::process::exit(entry());
```

`examples/add-numbers/provider` resolves `add_numbers_provider_main` from
`add_numbers_example.{dll,dylib,so}`. `examples/tokio-fetch/provider` resolves
`tokio_fetch_provider_main` from `tokio_fetch_example.{dll,dylib,so}`. The entry
point supports three operations, as shown in both examples:

- `--emit-manifest`: print `napi_oop::manifest::manifest_json()` for codegen.
- `NAPI_OOP_SOCKET` set: connect to the parent-provided socket and serve.
- otherwise: treat argv as a child command, spawn it with `NAPI_OOP_SOCKET`, and
  serve the child.

The npm runtime provides the Node side:

```ts
import { launchProviderSync, connectFromEnvSync, SOCKET_ENV } from 'napi-oop-runtime';
import { bind } from './generated/bindings';

const provider = process.env[SOCKET_ENV]
  ? connectFromEnvSync()                         // Rust parent spawned Node
  : launchProviderSync({ command: providerExe }); // Node parent spawns Rust

const native = bind(provider);
console.log(native.addNumbers(2, 3));
console.log(await native.multiplySlow(6, 7));
provider.close();
```

The data channel is a local named socket/pipe using napi-oop's framed
MessagePack protocol, never stdio.

## TypeScript runtime and codegen

`packages/runtime` publishes `napi-oop-runtime`. It contains:

- low-level async peer APIs (`launchProvider`, `connectFromEnv`, `Peer`);
- worker-backed sync APIs (`launchProviderSync`, `connectFromEnvSync`,
  `createSyncBinding`, `bindClasses`); and
- `napi-oop-codegen`, which runs a provider with `--emit-manifest` and writes a
  generated `bindings.ts`.

Usage:

```bash
napi-oop-codegen <provider-binary> <out-dir> [InterfaceName]
```

The generated binding mirrors native semantics: sync Rust functions/methods
return `T`; Rust `async fn` functions/methods return `Promise<T>` without
blocking the Node event loop. `#[napi]` classes are emitted as provider-bound TS
proxy classes. Constructors are called with `new`; sync members block; async
members and async getters return promises; functions returning classes are
wrapped as factories that produce the right proxy type.

## Supported Rust surface

The current e2e fixture covers these features in all three modes:

- **Functions**: sync functions and `async fn`; async calls dispatch concurrently
  in OOP and surface as `Promise<T>` in generated TS.
- **Classes**: `#[napi]` structs with `#[napi] impl`, constructors, methods,
  getters, `js_name` renames, class-returning methods, cross-class returns, and
  free-function factories.
- **Callbacks**: `impl Fn(T)` callback sugar and explicit
  `napi::ThreadsafeFunction<T>`. For in-process sync functions, `impl Fn(T)` is
  adapted to a synchronous napi `Function`; for async functions and explicit
  `ThreadsafeFunction`, calls use a real TSFN. OOP callbacks are fire-and-forget
  over the socket; default `ThreadsafeFunction<T>` is callee-handled, so JS sees
  `(err, value)`.
- **Errors**: `napi::Result<T>` returns `T` on success. `Err(napi::Error)` becomes
  a thrown exception in-process and an error reply that the OOP runtime throws or
  rejects. Panics in OOP dispatch are caught and returned as error replies.
- **Value types**: primitives, `String`, `bool`, numeric types, `Option<T>`
  (including omitted trailing optionals), `Vec<T>`, `#[napi(object)]` structs,
  `Buffer`/`Uint8Array`, full-precision `BigInt`/`bigint`, `Utf16String`, and
  opaque `External<T>` handles. `External<T>` and class instances live
  provider-side in OOP and are released when the JS handle is garbage-collected.
- **Host-injected values**: `Env`, `Object`, `AsyncBlockBuilder`, and tokio
  `spawn` helpers are shimmed sufficiently for compatible source patterns.

The manifest mapper falls back to `unknown` for Rust types it does not yet model.

## Examples

Install and build the workspace first:

```bash
npm install
npm run build -w napi-oop-runtime
```

Add-numbers, including sync, async, and both callback forms:

```bash
npm run build -w napi-oop-example-add-numbers
npm run start:node-parent -w napi-oop-example-add-numbers
npm run start:rust-parent -w napi-oop-example-add-numbers
```

Tokio-backed async example:

```bash
npm run build -w napi-oop-example-tokio-fetch
npm run start:node-parent -w napi-oop-example-tokio-fetch
npm run start:rust-parent -w napi-oop-example-tokio-fetch
```

In both OOP examples, the `build:types` script runs the generated provider host
with `--emit-manifest` and writes `ts/generated/bindings.ts`.

## Tests

Rust unit/integration tests:

```bash
cargo test --workspace
```

End-to-end tests:

```bash
npm install
npm run build -w napi-oop-runtime
cd tests/e2e
npm test
```

The e2e suite builds one fixture cdylib and proves all three hosting modes:

1. **in-proc**: Node `require()`s `fixture.node` through the real napi addon door;
2. **node-parent**: Node starts the Rust provider host with `launchProviderSync`;
3. **rust-parent**: the Rust provider host starts Node, and Node connects with
   `connectFromEnvSync`.
