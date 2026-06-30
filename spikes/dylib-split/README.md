# dylib-split spike

A throwaway experiment, **not part of the product build** (excluded from the root
Cargo workspace; built only by its own orchestrator and CI workflow).

## Question it answers

Can shared business logic be compiled **once** into a Rust dynamic library and
then linked by **two different thin wrappers** that ship side by side?

- `core/` — an `rlib` holding the "business logic" (the source of truth). Node-
  free and Node-API-free; its public Rust surface is the ABI the wrappers
  consume.
- `core-dyn/` — a `dylib` that re-exports `core`. This is the shared dynamic
  boundary: `core`'s code is statically linked into it once, and both wrappers
  link it dynamically so they share that single copy. Not built on musl.
- `node-addon/` — a `cdylib` (renamed to `index.node`) that Node loads
  in-process. The only artifact with a Node dependency. Each export is a thin
  forwarding shim into the core.
- `provider/` — a standalone executable used when a native host is the
  entrypoint. Same core, a different host, no Node dependency.

The wrappers select their core dependency per target: the shared `core-dyn`
dylib on dynamic-linking platforms, or the `core` rlib (statically linked) on
musl. Both wrappers call the core via the **native Rust ABI** (no C ABI, no
serialization) because all crates build together with one toolchain. The payoff
on dynamic platforms: one copy of the heavy logic, two hosting models, flat
package size.

## Mechanics under test

- `-C prefer-dynamic` so every artifact shares one dynamically-linked `std`
  (required, otherwise `panic_unwind only shows up once`). The toolchain's
  `std-<hash>` shared library is shipped alongside the artifacts.
- Sibling resolution of the shared libraries from the install directory
  (`$ORIGIN` on Linux, `@loader_path` on macOS, default search on Windows),
  validated by running from a foreign working directory with library-path env
  vars cleared.
- **musl fallback:** musl does not support the Rust `dylib` crate type (it ships
  no dynamic libstd), so the spike statically links the core into each wrapper
  there and produces two self-contained binaries instead (accepting size
  duplication on that one platform).

## Run it

```
node spikes/dylib-split/run-spike.mjs
```

The orchestrator builds, stages a self-contained `dist/`, applies macOS install
name / rpath fixups, and runs both the provider executable and
`node require('./index.node')`, asserting on their output. CI runs the same
script on Windows, macOS, glibc Linux, and musl (Alpine).
