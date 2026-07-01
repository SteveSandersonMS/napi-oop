// Codegen: turn the Rust-emitted type manifest into TypeScript binding sources.
//
// The Rust signatures are the IDL. The provider binary prints a manifest
// (`--emit-manifest`); this module renders it into a `.d.ts` describing both the
// async (Promise-returning) and sync bindings, plus a small `.js` that wires the
// runtime up. The caller imports the generated module instead of hand-writing
// interfaces — the same flow as napi-rs's generated `index.d.ts`.

import { camelToSnake } from './binding';


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

/** A `#[napi(object)]` value struct, rendered as a TS `interface`. */
export interface ObjectSignature {
  name: string;
  fieldNames: string[];
  fieldTypes: string[];
}

/** A `#[napi]` constant: a compile-time value exported at module scope, exactly
 *  as napi-rs emits `export const NAME`. The value is embedded from the manifest
 *  (constants never dispatch). */
export interface ConstSignature {
  jsName: string;
  rustName: string;
  tsType: string;
  value: unknown;
}

/** The provider's exposed surface. */
export interface Manifest {
  functions: FnSignature[];
  classes: ClassSignature[];
  objects: ObjectSignature[];
  constants: ConstSignature[];
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
    objects?: {
      name: string;
      field_names: string[];
      field_types: string[];
    }[];
    constants?: {
      js_name: string;
      rust_name: string;
      ts_type: string;
      value: unknown;
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
    objects: (raw.objects ?? []).map((o) => ({
      name: o.name,
      fieldNames: o.field_names,
      fieldTypes: o.field_types,
    })),
    constants: (raw.constants ?? []).map((c) => ({
      jsName: c.js_name,
      rustName: c.rust_name,
      tsType: c.ts_type,
      value: c.value,
    })),
  };
}

function paramList(sig: FnSignature): string {
  // An `Option<T>` param maps to `… | undefined | null`; napi-rs marks such a
  // param optional (`name?`) so callers may omit a trailing one. Mirror that so
  // the generated surface matches napi-rs and trailing optionals can be dropped.
  return sig.params
    .map((ty, i) => {
      const name = sig.paramNames[i] ?? `arg${i}`;
      const optional = ty.endsWith('| undefined | null');
      return `${name}${optional ? '?' : ''}: ${ty}`;
    })
    .join(', ');
}

/** The binding mirrors native semantics: a sync Rust fn returns its value `T`;
 *  an `async` Rust fn returns `Promise<T>`. A fn returning a class surfaces the
 *  proxy of the same name (wrapped in a `Promise` when the Rust fn is `async`). */
function returnType(f: FnSignature): string {
  return f.isAsync ? `Promise<${f.ret}>` : f.ret;
}

/** Array literal of the JS names of `async` fns (dispatched non-blocking). */
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

/** Object literal of `jsName -> rustName` for fns whose provider-side dispatch
 *  key isn't simply `camelToSnake(jsName)` — i.e. those declared with
 *  `#[napi(js_name = "…")]`, where the JS name diverges from the Rust name. The
 *  binding consults this map first so those calls reach the right function. */
function wireNamesLiteral(manifest: Manifest): string {
  const entries = manifest.functions
    .filter((f) => camelToSnake(f.jsName) !== f.rustName)
    .map((f) => `'${f.jsName}': '${f.rustName}'`);
  return `{ ${entries.join(', ')} }`;
}

/** Render a single self-contained `.ts` (interface + class proxies + factory).
 *  One binding, faithful to native: `bind(provider)` returns an object whose
 *  sync fns/methods block for their value and whose `async` fns/methods resolve
 *  a `Promise` without blocking the event loop. */
export function generateTs(manifest: Manifest, name = 'Bindings'): string {
  const classNames = new Set(manifest.classes.map((c) => c.name));
  // A function whose return type is a class name is a factory: its result is a
  // handle the binding wraps into the matching proxy.
  const factoryFns = manifest.functions.filter((f) => classNames.has(f.ret));
  const objectDecls = manifest.objects.map(generateObject).join('\n\n');
  const constDecls = (manifest.constants ?? [])
    .map((c) => `export const ${c.jsName}: ${c.tsType} = ${JSON.stringify(c.value)};`)
    .join('\n');
  const methods = manifest.functions
    .map((f) => `  ${f.jsName}(${paramList(f)}): ${returnType(f)};`)
    .join('\n');
  const classDecls = manifest.classes.map((c) => generateClass(c, manifest)).join('\n\n');
  const classProps = manifest.classes.map((c) => `  ${c.name}: typeof ${c.name};`).join('\n');
  const classMap = manifest.classes.length
    ? `{ ${manifest.classes.map((c) => c.name).join(', ')} } as unknown as Record<string, new (...a: unknown[]) => unknown>`
    : '{}';
  const factoryMap = factoryFns.length
    ? `{ ${factoryFns
        .map((f) => `${f.jsName}: { cls: ${f.ret}, isAsync: ${f.isAsync}, rustName: '${f.rustName}' }`)
        .join(', ')} } as unknown as Record<string, { cls: { __fromHandle(p: SyncProvider, h: unknown): unknown }; isAsync: boolean; rustName: string }>`
    : '{}';
  return `// Generated from the Rust #[napi] manifest. Do not edit.
import {
  createSyncBinding,
  bindClasses,
  type ExternalObject,
  type SyncProvider,
} from 'napi-oop-runtime';

export type { ExternalObject };

${objectDecls}${objectDecls ? '\n\n' : ''}${constDecls}${constDecls ? '\n\n' : ''}export interface ${name} {
${methods}
${classProps}
}

${classDecls}

const asyncFns: string[] = ${asyncFnsLiteral(manifest)};
const externalFns: string[] = ${externalFnsLiteral(manifest)};
const wireNames: Record<string, string> = ${wireNamesLiteral(manifest)};

export const bind = (provider: SyncProvider): ${name} =>
  bindClasses(
    createSyncBinding<${name}>(provider, asyncFns, externalFns, wireNames),
    provider,
    ${classMap},
    ${factoryMap}
  );
`;
}

/** A single `#[napi(object)]` value struct, rendered as a plain TS `interface`.
 *  Fields are by-value (camelCased on the Rust side already); the struct crosses
 *  the boundary as a MessagePack map, so callers get real field types. */
function generateObject(o: ObjectSignature): string {
  const fields = o.fieldNames
    .map((fieldName, i) => `  ${fieldName}: ${o.fieldTypes[i] ?? 'unknown'};`)
    .join('\n');
  return `export interface ${o.name} {\n${fields}\n}`;
}

/** A single TS class proxy. The instance holds a provider-side handle; each
 *  member round-trips it. Sync members block for their value; `async` members
 *  dispatch non-blocking and return a `Promise`. Class-typed returns wrap into
 *  the proxy and are tracked for GC-driven slab release. */
function generateClass(c: ClassSignature, manifest: Manifest): string {
  const classNames = new Set(manifest.classes.map((k) => k.name));
  const ctor = c.methods.find((m) => m.jsName === 'constructor');
  const ctorParams = ctor ? paramList(ctor) : '';
  const ctorArgs = ctor ? ctor.paramNames.join(', ') : '';
  const members = c.methods
    .filter((m) => m.jsName !== 'constructor')
    .map((m) => {
      const isClass = classNames.has(m.ret);
      const ret = m.isAsync ? `Promise<${m.ret}>` : m.ret;
      const argList = m.paramNames.length ? ', ' + m.paramNames.join(', ') : '';
      const verb = m.isAsync ? 'callAsync' : 'call';
      const call = `this.__provider.${verb}('${m.rustName}', [this.__handle${argList}])`;
      // Body that produces the member's value (sync) or awaits it (async).
      const body = m.isAsync
        ? isClass
          ? `const r = (await ${call}) as ExternalObject; return ${m.ret}.__fromHandle(this.__provider, r);`
          : `return (await ${call}) as ${m.ret};`
        : isClass
          ? `const r = ${call} as ExternalObject; return ${m.ret}.__fromHandle(this.__provider, r);`
          : `return ${call} as ${m.ret};`;
      if (m.isGetter) {
        // A getter can't be `async`; an async getter returns the Promise via an IIFE.
        return m.isAsync
          ? `  get ${m.jsName}(): ${ret} { return (async () => { ${body} })(); }`
          : `  get ${m.jsName}(): ${ret} { ${body} }`;
      }
      return m.isAsync
        ? `  async ${m.jsName}(${paramList(m)}): ${ret} { ${body} }`
        : `  ${m.jsName}(${paramList(m)}): ${ret} { ${body} }`;
    })
    .join('\n');
  return `export class ${c.name} {
  private __provider: SyncProvider;
  private __handle: ExternalObject;
  constructor(${ctorParams}${ctor && ctorArgs ? ', ' : ''}__provider?: SyncProvider) {
    this.__provider = __provider as SyncProvider;
    this.__handle = this.__provider.call('${ctor?.rustName ?? c.name + '.constructor'}', [${ctorArgs}]) as ExternalObject;
    this.__provider.trackExternal(this.__handle);
  }
  static __fromHandle(provider: SyncProvider, handle: ExternalObject): ${c.name} {
    const o = Object.create(${c.name}.prototype) as ${c.name};
    (o as unknown as { __provider: SyncProvider }).__provider = provider;
    (o as unknown as { __handle: ExternalObject }).__handle = handle;
    provider.trackExternal(handle);
    return o;
  }
${members}
}`;
}
