# napi-oop

Run `#[napi]`-annotated Rust **out of process** from Node, communicating over a
path-based named socket (never stdio). Either process may be the parent. See the
implementation plan for the architecture and roadmap.

> Status: the out-of-process path works end to end — Node launches a Rust
> provider as a child process, connects over a named socket, and `await`s calls
> to `#[napi]` functions (`await addNumbers(2, 3) === 5`). See the example.

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
npm start  -w @napi-oop/example-add-numbers   # run it -> addNumbers(2, 3) = 5
```
