// Ergonomic binding over a connected `Peer`.
//
// napi-rs exports a Rust `add_numbers` as JS `addNumbers`; we mirror that here.
// `createBinding<T>(peer)` returns a proxy whose every method call is forwarded
// to `peer.call`, converting the camelCase JS name back to the snake_case name
// the Rust registry advertises. Callers supply `T` for static typing.

import type { Peer } from './peer';

/** Convert a camelCase identifier to snake_case (`addNumbers` -> `add_numbers`). */
export function camelToSnake(name: string): string {
  return name.replace(/[A-Z]/g, (c) => `_${c.toLowerCase()}`);
}

/**
 * Wrap a peer as a typed object of async functions. Each property access yields
 * a function that calls the correspondingly-named Rust function and resolves
 * with its return value. Functions named in `externalFns` return an External
 * handle; the result is registered for GC-driven release of the provider slab.
 */
export function createBinding<T extends object>(
  peer: Peer,
  externalFns: readonly string[] = []
): T {
  const externalSet = new Set(externalFns);
  const cache = new Map<string, (...args: unknown[]) => Promise<unknown>>();
  return new Proxy({} as T, {
    get(_target, property) {
      if (typeof property !== 'string') return undefined;
      let fn = cache.get(property);
      if (!fn) {
        const wireName = camelToSnake(property);
        const tracks = externalSet.has(property);
        fn = async (...args: unknown[]) => {
          const result = await peer.call(wireName, args);
          if (tracks) peer.trackExternal(result);
          return result;
        };
        cache.set(property, fn);
      }
      return fn;
    },
  });
}

/** Minimal call surface async class proxies need: invoke a fn (resolving with
 *  its value) and register a returned handle for GC-driven slab release. */
export interface AsyncCaller {
  call(fn: string, args: unknown[]): Promise<unknown>;
  trackExternal(value: unknown): void;
}

/** Async counterpart of `bindClasses`: each generated class is reached via an
 *  awaited `create` factory, so we wrap them to inject the peer automatically.
 *  Callers write `await native.Counter.create(5)`. Free functions that return a
 *  class instance (`factories`) are wrapped to resolve the proxy too. */
export function bindClassesAsync<T extends object>(
  binding: T,
  caller: AsyncCaller,
  classes: Record<string, { create(...a: unknown[]): Promise<unknown> }>,
  factories: Record<string, { __fromHandle(c: AsyncCaller, h: unknown): unknown }> = {}
): T {
  const bound: Record<string, unknown> = {};
  for (const [name, Cls] of Object.entries(classes)) {
    bound[name] = { create: (...args: unknown[]) => Cls.create(...args, caller) };
  }
  for (const [name, Cls] of Object.entries(factories)) {
    const wireName = camelToSnake(name);
    bound[name] = async (...args: unknown[]) =>
      Cls.__fromHandle(caller, await caller.call(wireName, args));
  }
  return new Proxy(binding, {
    get: (t, p) =>
      typeof p === 'string' && p in bound ? bound[p] : (t as Record<PropertyKey, unknown>)[p],
  });
}
