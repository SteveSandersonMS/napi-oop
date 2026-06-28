# napi-pipe

A minimal [napi-rs](https://napi.rs/) example: a TypeScript entrypoint that calls
into a simple Rust function, `add_numbers`.

## Layout

- `src/rust/` — the Rust crate.
  - `src/rust/src/lib.rs` — the `add_numbers` function, exported to Node via `#[napi]`.
  - `src/rust/Cargo.toml` / `src/rust/build.rs` — crate config (builds a `cdylib`).
- `src/ts/` — the TypeScript source.
  - `src/ts/main.ts` — the entrypoint that calls `addNumbers`.
  - `src/ts/generated/` — generated native binding (`index.js`, `index.d.ts`, `*.node`).
- `dist/` — compiled JavaScript output (`tsc`, plus the copied `*.node`).
- `package.json` / `tsconfig.json` — build config.

The `src/ts/generated/` files and `dist/` are generated and git-ignored.

## Prerequisites

- Node.js
- Rust toolchain (`rustc` / `cargo`)

## Usage

```bash
npm install      # install @napi-rs/cli + TypeScript
npm run build    # Rust addon -> src/ts/generated/, tsc -> dist/, copy *.node -> dist/generated/
npm start        # node dist/main.js -> addNumbers(2, 3) = 5
```

Useful sub-scripts: `npm run build:native` (Rust only), `npm run build:ts`
(TypeScript only), and `npm run build:copy` (copy the native binary).
