// Synchronous calling variant.
//
// `peer.call` is async (returns a Promise) because the Node event loop drives
// the socket. To offer a blocking, synchronous API we move all I/O to a worker
// thread and block the main thread on `Atomics.wait`. The worker performs the
// real call, posts the result over a MessagePort, then notifies; the main
// thread wakes and pulls the (unbounded) result with `receiveMessageOnPort`.
//
// One call is in flight at a time, which matches synchronous semantics. JS
// callbacks ARE supported: the main thread assigns each a handle and keeps the
// function locally, sending only a {__napi_cb} marker to the worker. When the
// provider fires a callback the worker forwards it to the main thread, which
// drains the queued invocations between blocking calls (fire-and-forget, so
// deferring while the main thread is blocked is safe).

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

type Callback = (...args: unknown[]) => unknown;

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
  // Main-thread callback registry: handle -> JS function. The worker only ever
  // sees the handle and forwards invocations back here to fire.
  const callbacks = new Map<number, Callback>();
  let nextHandle = 1;

  const dispatchCallback = (handle: number, args: unknown[]): void => {
    const cb = callbacks.get(handle);
    if (!cb) return;
    try {
      cb(...args);
    } catch {
      // Fire-and-forget: callback errors are the caller's concern, not the wire's.
    }
  };

  // Drain and fire any pending callback invocations the worker queued, returning
  // the first non-callback (result/ready) message it finds.
  const drain = (): unknown => {
    for (;;) {
      const wrapper = receiveMessageOnPort(port1);
      if (!wrapper) return undefined;
      const msg = wrapper.message as { cb: true; handle: number; args: unknown[] } | object;
      if (msg && 'cb' in msg) {
        const inv = msg as { handle: number; args: unknown[] };
        dispatchCallback(inv.handle, inv.args);
        continue;
      }
      return msg;
    }
  };

  const waitForResult = (): unknown => {
    for (;;) {
      Atomics.wait(signal, 0, 0);
      Atomics.store(signal, 0, 0);
      const msg = drain();
      if (msg !== undefined) return msg;
    }
  };

  // Replace function args with {__napi_cb} markers, keeping the function local.
  const encodeArgs = (args: unknown[]): unknown[] =>
    args.map((a) => {
      if (typeof a !== 'function') return a;
      const handle = nextHandle++;
      callbacks.set(handle, a as Callback);
      return { __napi_cb: handle };
    });

  // Block until the worker has connected/launched and handshaked.
  const ready = waitForResult() as { ok: false; error: string } | { ready: true } | undefined;
  if (ready && 'ok' in ready && !ready.ok) {
    worker.terminate();
    throw new Error(ready.error);
  }

  return {
    call(fn, args) {
      if (closed) throw new Error('provider is closed');
      port1.postMessage({ fn, args: encodeArgs(args) });
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
