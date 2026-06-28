# napi-oop

Run `#[napi]`-annotated Rust **out of process** from Node, communicating over a
path-based named socket (never stdio). Either process may be the parent. See the
implementation plan for the architecture and roadmap.

> Status: early scaffolding. A working Phase 0 transport spike lives in `spike/`;
> the real runtime/macro crates are skeletons being filled in phase by phase.

## Repository layout

```
Cargo.toml                  # Cargo workspace
crates/
  napi-oop/                 # Rust runtime (transport, codec, wire, registry, peer)
  napi-oop-macro/           # the #[napi] attribute macro (in-proc / out-of-proc modes)
packages/
  runtime/                  # @napi-oop/runtime — Node-side runtime (TypeScript)
examples/
  add-numbers/              # example: TS entrypoint calling Rust add_numbers
spike/                      # Phase 0 throwaway transport spike (self-contained)
```

This is both a Cargo workspace (`crates/*`, `examples/*`) and an npm workspace
(`packages/*`, `examples/*`). The `spike/` is excluded from both.

## Build modes

`napi-oop-macro` exposes `#[napi]` with two cargo-feature build modes:

- `in-proc` (default) — behaves like a normal in-process napi-rs build.
- `out-of-proc` — emits out-of-process remoting glue.

(Both are pass-throughs today; codegen lands in a later phase.)

## Prerequisites

- Node.js
- Rust toolchain (`rustc` / `cargo`)

## Common tasks

```bash
cargo build                                   # build all Rust crates
npm install                                   # install npm workspaces
npm run build -w @napi-oop/runtime            # build the Node runtime package
npm run build -w @napi-oop/example-add-numbers  # build the example (napi + tsc)
npm start  -w @napi-oop/example-add-numbers   # run it -> addNumbers(2, 3) = 5
```

## Phase 0 spike

```bash
spike/run-node-parent.sh   # Node parent  -> Rust child
spike/run-rust-parent.sh   # Rust parent  -> Node child
```

Proves the named-socket transport with either side as the parent. See
`spike/README.md`.
