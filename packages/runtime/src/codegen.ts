// Codegen: turn the Rust-emitted type manifest into TypeScript binding sources.
//
// The Rust signatures are the IDL. The provider binary prints a manifest
// (`--emit-manifest`); this module renders it into a `.d.ts` describing both the
// async (Promise-returning) and sync bindings, plus a small `.js` that wires the
// runtime up. The caller imports the generated module instead of hand-writing
// interfaces — the same flow as napi-rs's generated `index.d.ts`.

/** A single function's signature, as emitted by `napi_oop::manifest`. */
export interface FnSignature {
  jsName: string;
  rustName: string;
  paramNames: string[];
  params: string[];
  ret: string;
  isAsync: boolean;
}

/** The provider's exposed surface. */
export interface Manifest {
  functions: FnSignature[];
}

/** Parse manifest JSON (camelCase keys come straight from serde rename). */
export function parseManifest(json: string): Manifest {
  const raw = JSON.parse(json) as {
    functions: {
      js_name: string;
      rust_name: string;
      param_names: string[];
      params: string[];
      ret: string;
      is_async: boolean;
    }[];
  };
  return {
    functions: raw.functions.map((f) => ({
      jsName: f.js_name,
      rustName: f.rust_name,
      paramNames: f.param_names,
      params: f.params,
      ret: f.ret,
      isAsync: f.is_async,
    })),
  };
}

function paramList(sig: FnSignature): string {
  return sig.params.map((ty, i) => `${sig.paramNames[i] ?? `arg${i}`}: ${ty}`).join(', ');
}

/** Async-binding return: always `Promise<T>`. */
function asyncRet(f: FnSignature): string {
  return `Promise<${f.ret}>`;
}

/** Sync-binding return: bare `T` for sync fns, but `Promise<T>` for async Rust
 *  fns — sync bindings must never hide a function's asynchrony. */
function syncRet(f: FnSignature): string {
  return f.isAsync ? `Promise<${f.ret}>` : f.ret;
}

/** Array literal of the JS names of async fns, for the sync-binding wrapper. */
function asyncFnsLiteral(manifest: Manifest): string {
  const names = manifest.functions.filter((f) => f.isAsync).map((f) => `'${f.jsName}'`);
  return `[${names.join(', ')}]`;
}

/** Render the `.d.ts`: an async interface (`Promise<T>`) and a sync one (`T`). */
export function generateDts(manifest: Manifest, name = 'Bindings'): string {
  const asyncMethods = manifest.functions
    .map((f) => `  ${f.jsName}(${paramList(f)}): ${asyncRet(f)};`)
    .join('\n');
  const syncMethods = manifest.functions
    .map((f) => `  ${f.jsName}(${paramList(f)}): ${syncRet(f)};`)
    .join('\n');
  return `// Generated from the Rust #[napi] manifest. Do not edit.
import type { Peer, SyncProvider } from '@napi-oop/runtime';

export interface ${name} {
${asyncMethods}
}

export interface ${name}Sync {
${syncMethods}
}

export declare function bind(peer: Peer): ${name};
export declare function bindSync(provider: SyncProvider): ${name}Sync;
`;
}

/** Render the `.js`: thin factories over the runtime's bindings. */
export function generateJs(manifest: Manifest): string {
  return `// Generated from the Rust #[napi] manifest. Do not edit.
const { createBinding, createSyncBinding } = require('@napi-oop/runtime');

const asyncFns = ${asyncFnsLiteral(manifest)};

exports.bind = (peer) => createBinding(peer);
exports.bindSync = (provider) => createSyncBinding(provider, asyncFns);
`;
}

/** Render a single self-contained `.ts` (interfaces + factories) — convenient
 *  when the consumer compiles the generated source with their own `tsc`. */
export function generateTs(manifest: Manifest, name = 'Bindings'): string {
  const asyncMethods = manifest.functions
    .map((f) => `  ${f.jsName}(${paramList(f)}): ${asyncRet(f)};`)
    .join('\n');
  const syncMethods = manifest.functions
    .map((f) => `  ${f.jsName}(${paramList(f)}): ${syncRet(f)};`)
    .join('\n');
  return `// Generated from the Rust #[napi] manifest. Do not edit.
import { createBinding, createSyncBinding, type Peer, type SyncProvider } from '@napi-oop/runtime';

export interface ${name} {
${asyncMethods}
}

export interface ${name}Sync {
${syncMethods}
}

const asyncFns = ${asyncFnsLiteral(manifest)};

export const bind = (peer: Peer): ${name} => createBinding<${name}>(peer);
export const bindSync = (provider: SyncProvider): ${name}Sync =>
  createSyncBinding<${name}Sync>(provider, asyncFns);
`;
}
