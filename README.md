# napi-oop

Compile the **same** `#[napi]`-annotated Rust source two ways: a native in-process
`.node` addon (real napi-rs) or an **out-of-process** provider that talks to Node
over a path-based named socket (never stdio). The source never changes — it keeps
using the `napi::…` path — only a cargo feature picks the mode.

> Status: both modes work end to end. **Out-of-process** has a **symmetric
> bootstrap** — either Node or Rust may be the parent that spawns the other.
> Node calls `#[napi]` functions **and classes** through one binding that mirrors
> native: sync Rust fns block for their value (`addNumbers(2, 3) === 5`) while
> `async` ones surface as non-blocking `Promise<T>`. Rust can expose stateful
> classes (constructor / methods / getters), pass value structs, `Buffer`,
> `BigInt` and opaque `External` handles, and invoke JS callbacks
> (`ThreadsafeFunction`). **In-process** builds an ordinary native addon via
> `@napi-rs/cli`. See the examples.

## Repository layout

```
Cargo.toml                  # Cargo workspace (excludes examples/native-add)
tsconfig.json               # TypeScript project references (monorepo root)
crates/
  napi-oop/                 # Rust runtime (transport, codec, wire, registry, peer, provider)
  napi-oop-macro/           # the #[napi] attribute macro (in-proc / out-of-proc modes)
  napi-oop-facade/          # crate published as `napi`: keeps source on the napi:: path,
                            #   re-exporting napi-rs (in-proc) or napi-oop shims (out-of-proc)
packages/
  runtime/                  # napi-oop-runtime — Node-side runtime (TypeScript)
examples/
  add-numbers/              # out-of-process: TS entrypoint calling Rust add_numbers
  tokio-fetch/              # out-of-process: concurrent tokio-backed async calls
  native-add/               # in-process: same source built as a native .node via napi-rs
```

This is both a Cargo workspace (`crates/*`, `examples/*`) and an npm workspace
(`packages/*`, `examples/*`).

## Build modes

`#[napi]` works in two cargo-feature build modes, both from unchanged source:

- `in-proc` (default) — a normal in-process napi-rs build. The facade re-exports
  real napi-rs, so `@napi-rs/cli` emits a native `.node` plus a typed loader.
- `out-of-proc` — emits the out-of-process remoting glue: a serde/MessagePack
  dispatch thunk registered with the runtime, served to Node over the socket.

## Prerequisites

- Node.js
- Rust toolchain (`rustc` / `cargo`)

## Common tasks

```bash
cargo build                                   # build all Rust crates
cargo test --workspace                        # run the Rust tests
npm install                                   # install npm workspaces
npm run build -w napi-oop-runtime            # build the Node runtime package
npm run build -w napi-oop-example-add-numbers  # build the example (cargo + tsc)

# Run it (symmetric bootstrap — either process can be the parent):
npm run start:node-parent -w napi-oop-example-add-numbers  # Node spawns Rust
npm run start:rust-parent -w napi-oop-example-add-numbers  # Rust spawns Node
```

Both print `addNumbers(2, 3) = 5`. The parent generates a named-socket path and
passes it to the child via the `NAPI_OOP_SOCKET` env var; the child connects
back. Rust stays the provider and Node the caller regardless of who is parent.

### In-process native addon

`examples/native-add` builds the **same** `#[napi]` source as a native `.node`:

```bash
npm run build -w napi-oop-example-native-add  # napi build (.node + .d.ts) then tsc
npm start     -w napi-oop-example-native-add  # prints addNumbers(2, 3) = 5
```

`@napi-rs/cli` generates the loader and `.d.ts` from the type-def metadata; the TS
entrypoint imports the typed binding. It lives outside the Cargo workspace because
its in-proc facade features are mutually exclusive with the out-of-proc examples.

### Generated TypeScript bindings

The Rust `#[napi]` signatures are the IDL. The provider prints a type manifest
(`<provider> --emit-manifest`); the `napi-oop-codegen` CLI turns it into a typed
`bindings.ts` exposing a single `bind(provider)` that mirrors native: sync fns
return `T`, `async` fns return `Promise<T>`. The caller never hand-writes
interfaces. The example regenerates them in its `build:types` step, then imports
`bind(provider)`.

### Async Rust + concurrency

`async fn` providers are detected from the `async` keyword and dispatched
concurrently across a fixed worker pool (sized to `available_parallelism`), so
overlapping calls overlap their latency without spawning a thread per request.
The manifest marks them async, so they surface as `Promise<T>` from the single
binding: asynchrony is never hidden behind a blocking call. `multiplySlow` in
the example proves two 200ms calls finish in ~200ms.

There's also a **tokio** example: enable napi-oop's `tokio` feature so `block_on`
runs on a shared multi-thread tokio runtime, and real tokio futures (timers, IO)
work. `examples/tokio-fetch` runs three `tokio::time::sleep(200ms)` calls in
~200ms total.

### Callbacks (ThreadsafeFunction)

Providers can invoke JS callbacks fire-and-forget, matching napi-rs semantics:
accept either an `impl Fn(..)` parameter or an explicit `ThreadsafeFunction<T>`,
and the runtime routes invocations back over the socket. Callbacks fired during a
blocking sync call are drained before the call returns; those fired while the main
thread is idle arrive on the event loop.

### Classes & value types

The surface mirrors napi-rs beyond free functions. A `#[napi]` struct with an
`impl` block becomes a JS class: `#[napi(constructor)]`, instance methods,
`#[napi(getter)]` accessors, and per-item `#[napi(js_name = "…")]` renames all
remoting to the provider, which owns the live Rust instance. Data crosses the
socket as the same types napi-rs supports — `#[napi(object)]` value structs,
`Buffer`, `BigInt`, `Option<T>`/trailing optionals, `Vec<T>` — plus opaque
`External<T>` handles that stay in the provider and travel by reference. The
generated `bindings.ts` types classes and value structs alongside functions.

### Process lifecycle

The two processes mirror an in-process addon: native code never disappears
mid-call. The parent (either side) spawns the child with the socket path; the
provider serves until the socket reaches EOF, then exits — so the Node side owns
graceful shutdown and the provider follows. napi-oop deliberately leaves *which*
side is the parent, signal handling, and process exit to the application.



