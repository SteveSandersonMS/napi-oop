# napi-oop

Run `#[napi]`-annotated Rust **out of process** from Node, communicating over a
path-based named socket (never stdio). Either process may be the parent. See the
implementation plan for the architecture and roadmap.

> Status: the out-of-process path works end to end, with a **symmetric
> bootstrap** — either Node or Rust may be the parent that spawns the other.
> Node `await`s calls to `#[napi]` functions over a named socket
> (`await addNumbers(2, 3) === 5`), or calls them **synchronously** via a
> worker-backed blocking variant. See the example.

## Repository layout

```
Cargo.toml                  # Cargo workspace
tsconfig.json               # TypeScript project references (monorepo root)
crates/
  napi-oop/                 # Rust runtime (transport, codec, wire, registry, peer, provider)
  napi-oop-macro/           # the #[napi] attribute macro (in-proc / out-of-proc modes)
packages/
  runtime/                  # @napi-oop/runtime — Node-side runtime (TypeScript)
examples/
  add-numbers/              # example: TS entrypoint calling out-of-process Rust add_numbers
```

This is both a Cargo workspace (`crates/*`, `examples/*`) and an npm workspace
(`packages/*`, `examples/*`).

## Build modes

`napi-oop-macro` exposes `#[napi]` with two cargo-feature build modes:

- `in-proc` (default) — behaves like a normal in-process napi-rs build
  (pass-through today).
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
npm run build -w @napi-oop/runtime            # build the Node runtime package
npm run build -w @napi-oop/example-add-numbers  # build the example (cargo + tsc)

# Run it (symmetric bootstrap — either process can be the parent):
npm run start:node-parent -w @napi-oop/example-add-numbers  # Node spawns Rust
npm run start:rust-parent -w @napi-oop/example-add-numbers  # Rust spawns Node
npm run start:sync        -w @napi-oop/example-add-numbers  # blocking/sync call
```

Both print `addNumbers(2, 3) = 5`. The parent generates a named-socket path and
passes it to the child via the `NAPI_OOP_SOCKET` env var; the child connects
back. Rust stays the provider and Node the caller regardless of who is parent.

### Generated TypeScript bindings

The Rust `#[napi]` signatures are the IDL. The provider prints a type manifest
(`<provider> --emit-manifest`); the `napi-oop-codegen` CLI turns it into a typed
`bindings.ts` (both an async `Promise<T>` interface and a sync one), so the
caller never hand-writes interfaces. The example regenerates them in its
`build:types` step, then imports `bind(peer)` / `bindSync(provider)`.

