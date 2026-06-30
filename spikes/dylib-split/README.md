# dylib-split spike

A throwaway experiment, **not part of the product build** (excluded from the root
Cargo workspace; built only by its own orchestrator and CI workflow).

## Question it answers

Can shared business logic be compiled **once** into a Rust dynamic library and
then linked by **two different thin wrappers** that ship side by side?

- `core/` — a `dylib` holding the "business logic". It is Node-free and
  Node-API-free; its public Rust surface is the ABI the wrappers consume.
- `node-addon/` — a `cdylib` (renamed to `index.node`) that Node loads
  in-process. The only artifact with a Node dependency. Each export is a thin
  forwarding shim into `core`.
- `provider/` — a standalone executable used when a native host is the
  entrypoint. Same `core`, a different host, no Node dependency.

Both wrappers call `core` via the **native Rust ABI** (no C ABI, no
serialization) because all three crates build together with one toolchain. The
payoff: one copy of the heavy logic, two hosting models, flat package size.

## Mechanics under test

- `-C prefer-dynamic` so every artifact shares one dynamically-linked `std`
  (required, otherwise `panic_unwind only shows up once`). The toolchain's
  `std-<hash>` shared library is shipped alongside the artifacts.
- Sibling resolution of the shared libraries from the install directory
  (`$ORIGIN` on Linux, `@loader_path` on macOS, default search on Windows),
  validated by running from a foreign working directory with library-path env
  vars cleared.

## Run it

```
node spikes/dylib-split/run-spike.mjs
```

The orchestrator builds, stages a self-contained `dist/`, applies macOS install
name / rpath fixups, and runs both the provider executable and
`node require('./index.node')`, asserting on their output. CI runs the same
script on Windows, macOS, glibc Linux, and musl (Alpine).
