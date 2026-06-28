// Synchronous calling variant.
//
// `peer.call` is async (returns a Promise) because the Node event loop drives
// the socket. To offer a blocking, synchronous API we move all I/O to a worker
// thread and block the main thread on `Atomics.wait`. The worker performs the
// real call, posts the result over a MessagePort, then notifies; the main
// thread wakes and pulls the (unbounded) result with `receiveMessageOnPort`.
//
// One call is in flight at a time, which matches synchronous semantics. Nested
// callbacks (Rust calling back into Node while we block) are a Phase 7 concern;
// the message pump here will be extended to service them.

import { receiveMessageOnPort, MessageChannel, Worker } from 'worker_threads';
import { join } from 'path';

import { camelToSnake } from './binding';

/** A synchronous handle to the provider; calls block until the result is ready. */
export interface SyncProvider {
  /** Call a function synchronously, returning its value (or throwing on error). */
  call(fn: string, args: unknown[]): unknown;
  /** Shut down the worker and underlying provider. */
  close(): void;
}

/** Options for [`launchProviderSync`]. */
export interface LaunchSyncOptions {
  command: string;
  args?: string[];
  socketPath?: string;
}

function spawnSyncProvider(mode: 'launch' | 'connectEnv', opts: LaunchSyncOptions): SyncProvider {
  // [1] signal: 0 = waiting, 1 = result ready. The worker bumps + notifies.
  const signal = new Int32Array(new SharedArrayBuffer(4));
  const { port1, port2 } = new MessageChannel();
  const worker = new Worker(join(__dirname, 'sync-worker.js'), {
    workerData: { signal, mode, port: port2, ...opts },
    transferList: [port2],
  });
  // Don't let the worker keep the process alive after the caller is done.
  worker.unref();

  let closed = false;

  const waitForResult = (): unknown => {
    Atomics.wait(signal, 0, 0);
    Atomics.store(signal, 0, 0);
    const msg = receiveMessageOnPort(port1)?.message as
      | { ready: true }
      | { ok: true; result: unknown }
      | { ok: false; error: string }
      | undefined;
    return msg;
  };

  // Block until the worker has connected/launched and handshaked.
  const ready = waitForResult() as { ok: false; error: string } | { ready: true } | undefined;
  if (ready && 'ok' in ready && !ready.ok) {
    worker.terminate();
    throw new Error(ready.error);
  }

  return {
    call(fn, args) {
      if (closed) throw new Error('provider is closed');
      port1.postMessage({ fn, args });
      const msg = waitForResult() as { ok: true; result: unknown } | { ok: false; error: string };
      if (msg.ok) return msg.result;
      throw new Error(msg.error);
    },
    close() {
      if (closed) return;
      closed = true;
      port1.postMessage({ close: true });
      // Wait for the worker to finish provider shutdown + socket cleanup before
      // terminating it, so no socket files are leaked.
      Atomics.wait(signal, 0, 0);
      worker.terminate();
    },
  };
}

/** Launch a provider child and return a synchronous handle (Node is parent). */
export function launchProviderSync(options: LaunchSyncOptions): SyncProvider {
  return spawnSyncProvider('launch', options);
}

/** Connect synchronously as a child, using the `NAPI_OOP_SOCKET` env var. */
export function connectFromEnvSync(): SyncProvider {
  return spawnSyncProvider('connectEnv', { command: '' });
}

/**
 * Wrap a [`SyncProvider`] as a typed object of synchronous functions. Sync calls
 * block and return the value directly. Functions whose names are in `asyncFns`
 * are Rust `async` fns: sync bindings must not hide their asynchrony, so the
 * (still-blocking) result is wrapped in a resolved `Promise<T>` to honor the
 * `Promise`-typed signature.
 */
export function createSyncBinding<T extends object>(
  provider: SyncProvider,
  asyncFns: readonly string[] = []
): T {
  const asyncSet = new Set(asyncFns);
  const cache = new Map<string, (...args: unknown[]) => unknown>();
  return new Proxy({} as T, {
    get(_target, property) {
      if (typeof property !== 'string') return undefined;
      let fn = cache.get(property);
      if (!fn) {
        const wireName = camelToSnake(property);
        const isAsync = asyncSet.has(property);
        fn = (...args: unknown[]) =>
          isAsync ? Promise.resolve(provider.call(wireName, args)) : provider.call(wireName, args);
        cache.set(property, fn);
      }
      return fn;
    },
  });
}
