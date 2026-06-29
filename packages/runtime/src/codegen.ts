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

export interface MethodSignature extends FnSignature {
  isGetter: boolean;
}

export interface ClassSignature {
  name: string;
  methods: MethodSignature[];
}

/** The provider's exposed surface. */
export interface Manifest {
  functions: FnSignature[];
  classes: ClassSignature[];
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
    classes?: {
      name: string;
      methods: {
        js_name: string;
        rust_name: string;
        param_names: string[];
        params: string[];
        ret: string;
        is_async: boolean;
        is_getter: boolean;
      }[];
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
    classes: (raw.classes ?? []).map((c) => ({
      name: c.name,
      methods: c.methods.map((m) => ({
        jsName: m.js_name,
        rustName: m.rust_name,
        paramNames: m.param_names,
        params: m.params,
        ret: m.ret,
        isAsync: m.is_async,
        isGetter: m.is_getter,
      })),
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

/** Array literal of fns returning a top-level External, for GC-driven release. */
function externalFnsLiteral(manifest: Manifest): string {
  const names = manifest.functions
    .filter((f) => f.ret === 'ExternalObject')
    .map((f) => `'${f.jsName}'`);
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
import type { ExternalObject, Peer, SyncProvider } from 'napi-oop-runtime';

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
const { createBinding, createSyncBinding } = require('napi-oop-runtime');

const asyncFns = ${asyncFnsLiteral(manifest)};
const externalFns = ${externalFnsLiteral(manifest)};

exports.bind = (peer) => createBinding(peer, externalFns);
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
  const classDecls = manifest.classes.map((c) => generateClass(c, manifest)).join('\n\n');
  const classProps = manifest.classes.map((c) => `  ${c.name}: typeof ${c.name};`).join('\n');
  const classMap = manifest.classes.length
    ? `{ ${manifest.classes.map((c) => `${c.name}`).join(', ')} } as unknown as Record<string, new (...a: unknown[]) => unknown>`
    : '{}';
  return `// Generated from the Rust #[napi] manifest. Do not edit.
import {
  createBinding,
  createSyncBinding,
  bindClasses,
  type ExternalObject,
  type Peer,
  type SyncProvider,
} from 'napi-oop-runtime';

export type { ExternalObject };

export interface ${name} {
${asyncMethods}
}

export interface ${name}Sync {
${syncMethods}
${classProps}
}

${classDecls}

const asyncFns: string[] = ${asyncFnsLiteral(manifest)};
const externalFns: string[] = ${externalFnsLiteral(manifest)};

export const bind = (peer: Peer): ${name} => createBinding<${name}>(peer, externalFns);
export const bindSync = (provider: SyncProvider): ${name}Sync =>
  bindClasses(createSyncBinding<${name}Sync>(provider, asyncFns), provider, ${classMap});
`;
}

/** TS class proxy: instances hold a provider-side handle; methods round-trip it. */
function generateClass(c: ClassSignature, manifest: Manifest): string {
  const classNames = new Set(manifest.classes.map((k) => k.name));
  const ctor = c.methods.find((m) => m.jsName === 'constructor');
  const ctorParams = ctor ? paramList(ctor) : '';
  const ctorArgs = ctor ? ctor.paramNames.join(', ') : '';
  const members = c.methods
    .filter((m) => m.jsName !== 'constructor')
    .map((m) => {
      const ret = m.isAsync ? `Promise<${m.ret}>` : m.ret;
      const wrap = classNames.has(m.ret) ? `${m.ret}.__fromHandle(this.__provider, r)` : `r`;
      const call = `this.__provider.call('${m.rustName}', [this.__handle${m.paramNames.length ? ', ' + m.paramNames.join(', ') : ''}])`;
      const body = classNames.has(m.ret)
        ? `const r = ${call} as ExternalObject; return ${wrap};`
        : `return ${call} as ${ret};`;
      if (m.isGetter) return `  get ${m.jsName}(): ${m.ret} { ${body} }`;
      return `  ${m.jsName}(${paramList(m)}): ${ret} { ${body} }`;
    })
    .join('\n');
  return `export class ${c.name} {
  private __provider: SyncProvider;
  private __handle: ExternalObject;
  constructor(${ctorParams}${ctor && ctorArgs ? ', ' : ''}__provider?: SyncProvider) {
    this.__provider = __provider as SyncProvider;
    this.__handle = this.__provider.call('${ctor?.rustName ?? c.name + '.constructor'}', [${ctorArgs}]) as ExternalObject;
  }
  static __fromHandle(provider: SyncProvider, handle: ExternalObject): ${c.name} {
    const o = Object.create(${c.name}.prototype) as ${c.name};
    (o as unknown as { __provider: SyncProvider }).__provider = provider;
    (o as unknown as { __handle: ExternalObject }).__handle = handle;
    return o;
  }
${members}
}`;
}
