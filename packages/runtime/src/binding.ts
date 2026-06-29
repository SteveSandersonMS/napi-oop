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
