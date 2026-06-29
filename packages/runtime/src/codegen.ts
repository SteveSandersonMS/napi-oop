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
  const classNames = new Set(manifest.classes.map((c) => c.name));
  // A function whose return type is a class name is a factory: its result is a
  // handle the binding wraps into the matching proxy.
  const factoryFns = manifest.functions.filter((f) => classNames.has(f.ret));
  // Per-mode return type for a function: class returns become the proxy variant.
  const fnRet = (f: FnSignature, mode: 'sync' | 'async'): string => {
    if (classNames.has(f.ret)) {
      const proxy = mode === 'async' ? `${f.ret}Async` : f.ret;
      return mode === 'async' || f.isAsync ? `Promise<${proxy}>` : proxy;
    }
    return mode === 'async' ? asyncRet(f) : syncRet(f);
  };
  const asyncMethods = manifest.functions
    .map((f) => `  ${f.jsName}(${paramList(f)}): ${fnRet(f, 'async')};`)
    .join('\n');
  const syncMethods = manifest.functions
    .map((f) => `  ${f.jsName}(${paramList(f)}): ${fnRet(f, 'sync')};`)
    .join('\n');
  const syncClassDecls = manifest.classes.map((c) => generateClass(c, manifest, 'sync')).join('\n\n');
  const asyncClassDecls = manifest.classes.map((c) => generateClass(c, manifest, 'async')).join('\n\n');
  const classDecls = [syncClassDecls, asyncClassDecls].filter(Boolean).join('\n\n');
  const syncClassProps = manifest.classes.map((c) => `  ${c.name}: typeof ${c.name};`).join('\n');
  const asyncClassProps = manifest.classes.map((c) => `  ${c.name}: typeof ${c.name}Async;`).join('\n');
  const syncClassMap = manifest.classes.length
    ? `{ ${manifest.classes.map((c) => `${c.name}`).join(', ')} } as unknown as Record<string, new (...a: unknown[]) => unknown>`
    : '{}';
  const asyncClassMap = manifest.classes.length
    ? `{ ${manifest.classes.map((c) => `${c.name}: ${c.name}Async`).join(', ')} } as unknown as Record<string, { create(...a: unknown[]): Promise<unknown> }>`
    : '{}';
  const syncFactoryMap = factoryFns.length
    ? `{ ${factoryFns.map((f) => `${f.jsName}: ${f.ret}`).join(', ')} } as unknown as Record<string, { __fromHandle(p: SyncProvider, h: unknown): unknown }>`
    : '{}';
  const asyncFactoryMap = factoryFns.length
    ? `{ ${factoryFns.map((f) => `${f.jsName}: ${f.ret}Async`).join(', ')} } as unknown as Record<string, { __fromHandle(c: AsyncCaller, h: unknown): unknown }>`
    : '{}';
  return `// Generated from the Rust #[napi] manifest. Do not edit.
import {
  createBinding,
  createSyncBinding,
  bindClasses,
  bindClassesAsync,
  type AsyncCaller,
  type ExternalObject,
  type Peer,
  type SyncProvider,
} from 'napi-oop-runtime';

export type { ExternalObject };

export interface ${name} {
${asyncMethods}
${asyncClassProps}
}

export interface ${name}Sync {
${syncMethods}
${syncClassProps}
}

${classDecls}

const asyncFns: string[] = ${asyncFnsLiteral(manifest)};
const externalFns: string[] = ${externalFnsLiteral(manifest)};

export const bind = (peer: Peer): ${name} =>
  bindClassesAsync(createBinding<${name}>(peer, externalFns), peer, ${asyncClassMap}, ${asyncFactoryMap});
export const bindSync = (provider: SyncProvider): ${name}Sync =>
  bindClasses(createSyncBinding<${name}Sync>(provider, asyncFns), provider, ${syncClassMap}, ${syncFactoryMap});
`;
}

/** TS class proxies: instances hold a provider-side handle; methods round-trip
 *  it. Sync proxies block; async proxies return Promises and are constructed via
 *  an awaited `create` factory (a ctor can't await). Class-typed returns wrap
 *  into the matching proxy and are tracked for GC-driven slab release. */
function generateClass(c: ClassSignature, manifest: Manifest, mode: 'sync' | 'async'): string {
  const classNames = new Set(manifest.classes.map((k) => k.name));
  const suffix = mode === 'async' ? 'Async' : '';
  const tn = (n: string) => (classNames.has(n) ? `${n}${suffix}` : n);
  const ct = mode === 'async' ? 'AsyncCaller' : 'SyncProvider';
  const name = `${c.name}${suffix}`;
  const ctor = c.methods.find((m) => m.jsName === 'constructor');
  const ctorParams = ctor ? paramList(ctor) : '';
  const ctorArgs = ctor ? ctor.paramNames.join(', ') : '';
  const members = c.methods
    .filter((m) => m.jsName !== 'constructor')
    .map((m) => {
      const isClass = classNames.has(m.ret);
      const wrapT = isClass ? tn(m.ret) : m.ret;
      const ret = mode === 'async' || m.isAsync ? `Promise<${wrapT}>` : wrapT;
      const call = `this.__provider.call('${m.rustName}', [this.__handle${m.paramNames.length ? ', ' + m.paramNames.join(', ') : ''}])`;
      if (mode === 'async') {
        const body = isClass
          ? `const r = (await ${call}) as ExternalObject; return ${tn(m.ret)}.__fromHandle(this.__provider, r);`
          : `return (await ${call}) as ${wrapT};`;
        if (m.isGetter) return `  get ${m.jsName}(): ${ret} { return (async () => { ${body} })(); }`;
        return `  async ${m.jsName}(${paramList(m)}): ${ret} { ${body} }`;
      }
      const wrap = isClass ? `${tn(m.ret)}.__fromHandle(this.__provider, r)` : `r`;
      const body = isClass
        ? `const r = ${call} as ExternalObject; return ${m.isAsync ? `Promise.resolve(${wrap})` : wrap};`
        : `return ${call} as ${ret};`;
      if (m.isGetter) return `  get ${m.jsName}(): ${ret} { ${body} }`;
      return `  ${m.jsName}(${paramList(m)}): ${ret} { ${body} }`;
    })
    .join('\n');
  const fromHandle = `  static __fromHandle(provider: ${ct}, handle: ExternalObject): ${name} {
    const o = Object.create(${name}.prototype) as ${name};
    (o as unknown as { __provider: ${ct} }).__provider = provider;
    (o as unknown as { __handle: ExternalObject }).__handle = handle;
    provider.trackExternal(handle);
    return o;
  }`;
  if (mode === 'async') {
    return `export class ${name} {
  private __provider!: AsyncCaller;
  private __handle!: ExternalObject;
  static async create(${ctorParams}${ctor && ctorArgs ? ', ' : ''}__provider?: AsyncCaller): Promise<${name}> {
    const h = (await (__provider as AsyncCaller).call('${ctor?.rustName ?? c.name + '.constructor'}', [${ctorArgs}])) as ExternalObject;
    return ${name}.__fromHandle(__provider as AsyncCaller, h);
  }
${fromHandle}
${members}
}`;
  }
  return `export class ${name} {
  private __provider: SyncProvider;
  private __handle: ExternalObject;
  constructor(${ctorParams}${ctor && ctorArgs ? ', ' : ''}__provider?: SyncProvider) {
    this.__provider = __provider as SyncProvider;
    this.__handle = this.__provider.call('${ctor?.rustName ?? c.name + '.constructor'}', [${ctorArgs}]) as ExternalObject;
    this.__provider.trackExternal(this.__handle);
  }
${fromHandle}
${members}
}`;
}
