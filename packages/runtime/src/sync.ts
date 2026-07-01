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
import { diag, diagTrace } from './diag';
import { SOCKET_ENV } from './index';

/**
 * A handle to the out-of-process provider that mirrors native semantics:
 * synchronous Rust fns block the main thread for their value, while `async`
 * Rust fns dispatch without blocking and resolve a `Promise`. A worker thread
 * owns the socket; sync calls block on `Atomics.wait`, async calls and callback
 * invocations flow over a separate event-loop `MessagePort`.
 */
export interface SyncProvider {
  /** Call a sync Rust fn, blocking until the result is ready (or throwing). */
  call(fn: string, args: unknown[]): unknown;
  /** Call an `async` Rust fn without blocking the event loop; resolves the value. */
  callAsync(fn: string, args: unknown[]): Promise<unknown>;
  /** Register a returned handle for GC-driven provider-side slab release. */
  trackExternal(value: unknown): void;
  /** Shut down the worker and underlying provider. */
  close(): void;
}

/** Return the `__napi_ext` token if `v` is an External handle marker. */
function externalToken(v: unknown): number | undefined {
  if (v && typeof v === 'object' && '__napi_ext' in v) {
    const t = (v as { __napi_ext: unknown }).__napi_ext;
    return typeof t === 'number' ? t : undefined;
  }
  return undefined;
}

/** Options for [`launchProviderSync`]. */
export interface LaunchSyncOptions {
  command: string;
  args?: string[];
  socketPath?: string;
}

type Callback = (...args: unknown[]) => unknown;

interface AsyncResult {
  asyncResult: true;
  id: number;
  ok: boolean;
  result?: unknown;
  error?: string;
}
interface CallbackInvoke {
  cb: true;
  handle: number;
  args: unknown[];
  /** Monotonic per-provider fire order, assigned in the worker. */
  seq: number;
}
interface CallbackRelease {
  cbRelease: true;
  handle: number;
}
interface ProviderClosed {
  providerClosed: true;
}

function spawnSyncProvider(mode: 'launch' | 'connectEnv', opts: LaunchSyncOptions): SyncProvider {
  // [1] signal: 0 = waiting, 1 = result ready. The worker bumps + notifies.
  const signal = new Int32Array(new SharedArrayBuffer(4));
  // `port1`/`port2`: the synchronous channel, drained under `Atomics.wait`.
  const { port1, port2 } = new MessageChannel();
  // `asyncMain`/`asyncWorker`: the event-loop channel for non-blocking `async`
  // calls, their results, and callback invocations.
  const { port1: asyncMain, port2: asyncWorker } = new MessageChannel();
  const worker = new Worker(join(__dirname, 'sync-worker.js'), {
    workerData: { signal, mode, port: port2, asyncPort: asyncWorker, ...opts },
    transferList: [port2, asyncWorker],
  });
  // Don't let the worker keep the process alive after the caller is done.
  worker.unref();

  let closed = false;
  let workerTerminated = false;
  // Main-thread callback registry: handle -> JS function. The worker only ever
  // sees the handle and forwards invocations back here to fire.
  const callbacks = new Map<number, Callback>();
  let nextHandle = 1;

  // Pending non-blocking `async` calls, keyed by id. The async port is ref'd
  // while calls are outstanding *or* while a callback the provider still holds is
  // live, so a long-running provider activity (e.g. a server's accept callback)
  // keeps the process alive — matching how an in-process `ThreadsafeFunction` is
  // ref'd by default — while a process with no outstanding work still exits.
  const pending = new Map<number, { resolve: (v: unknown) => void; reject: (e: Error) => void }>();
  let nextCallId = 1;
  let refCount = 0;
  asyncMain.unref();
  const refAsync = (): void => {
    if (refCount++ === 0) asyncMain.ref();
  };
  const unrefAsync = (): void => {
    if (--refCount === 0) asyncMain.unref();
  };

  // GC-driven release: when a tracked External/handle object is collected, ask
  // the worker to free the provider-side slab entry (fire-and-forget).
  const externals = new FinalizationRegistry<number>((token) => {
    if (!closed) asyncMain.postMessage({ release: true, token });
  });

  const dispatchCallback = (handle: number, args: unknown[]): void => {
    const cb = callbacks.get(handle);
    if (!cb) return;
    try {
      cb(...args);
    } catch {
      // Fire-and-forget: callback errors are the caller's concern, not the wire's.
    }
  };

  // Global FIFO callback ordering. The worker tags every invocation with a
  // monotonic `seq` (its fire order) and splits delivery across two ports: the
  // sync port (drained synchronously under `Atomics.wait` while a blocking call
  // is in flight) and the async port (the event loop, when the main thread is
  // idle). Those two channels have different latencies, so a callback fired
  // later can arrive first — e.g. one fired during a blocking call is drained off
  // the sync port before an earlier one still queued on the async port. Dispatch
  // strictly in `seq` order, buffering any that arrive ahead of their turn, so a
  // caller observes callbacks in the exact order the provider fired them —
  // matching an in-process `ThreadsafeFunction`'s single FIFO queue.
  const cbReorder = new Map<number, { handle: number; args: unknown[] }>();
  let nextCbSeq = 1;
  const deliverCallback = (seq: number, handle: number, args: unknown[]): void => {
    cbReorder.set(seq, { handle, args });
    while (cbReorder.has(nextCbSeq)) {
      const inv = cbReorder.get(nextCbSeq)!;
      cbReorder.delete(nextCbSeq);
      // Advance before dispatching so a callback that reenters (fires the next
      // one synchronously via a nested sync call) sees a consistent cursor.
      nextCbSeq++;
      dispatchCallback(inv.handle, inv.args);
    }
  };

  // The provider connection has gone away. Release all callback keep-alive refs
  // (a dead provider can't fire them), fail outstanding async calls, and mark
  // the handle closed so further calls throw rather than block forever.
  const onProviderGone = (): void => {
    if (closed) return;
    closed = true;
    callbacks.clear();
    cbReorder.clear();
    refCount = 0;
    asyncMain.unref();
    for (const p of pending.values()) p.reject(new Error('provider is closed'));
    pending.clear();
  };

  // Event-loop delivery of async results and callbacks. Fires whenever the main
  // thread is free; while a sync call blocks, these queue and run after it.
  // Callback invocations are dispatched in global `seq` order.
  asyncMain.on('message', (msg: AsyncResult | CallbackInvoke | CallbackRelease | ProviderClosed) => {
    if ('providerClosed' in msg) {
      // The provider connection dropped (e.g. it crashed or was signalled). Its
      // held callbacks can never fire again, so release every keep-alive ref and
      // let the event loop drain. Pending async calls are failed; further calls
      // throw `provider is closed`.
      onProviderGone();
      return;
    }
    if ('cbRelease' in msg) {
      // The provider dropped this callback; drop our entry and release the
      // keep-alive ref taken when it was sent. Guard on delete so a stray or
      // duplicate release can't unbalance the ref count.
      if (callbacks.delete(msg.handle)) unrefAsync();
      return;
    }
    if ('cb' in msg) {
      deliverCallback(msg.seq, msg.handle, msg.args);
      return;
    }
    const p = pending.get(msg.id);
    if (!p) return;
    pending.delete(msg.id);
    unrefAsync();
    if (msg.ok) p.resolve(msg.result);
    else p.reject(new Error(msg.error));
  });

  // Drain the sync port. While a blocking sync call is in flight the worker may
  // post callback invocations here (so they fire synchronously, before the call
  // returns); dispatch those and keep going. Each result carries the `syncId` of
  // the call it belongs to. A synchronous callback can *reenter* with another
  // sync call while the outer one is still completing, so two results may race on
  // this port — return only the one for `expectedId` and buffer any other by its
  // id so the call awaiting it finds it (rather than mis-delivering it here).
  const syncBuffer = new Map<number, unknown>();
  const drain = (expectedId: number): unknown => {
    for (;;) {
      const wrapper = receiveMessageOnPort(port1);
      if (!wrapper) return undefined;
      const msg = wrapper.message as CallbackInvoke | (Record<string, unknown> & { syncId?: number });
      if (msg && 'cb' in msg) {
        const inv = msg as CallbackInvoke;
        diag('main-cb-dispatch', { handle: inv.handle });
        deliverCallback(inv.seq, inv.handle, inv.args);
        continue;
      }
      const sid = (msg as { syncId?: number }).syncId ?? 0;
      if (sid === expectedId) return msg;
      // A result for a *different* in-flight sync call (reentrancy): buffer it by
      // id. Without this the outer/inner sync calls could swap results, since the
      // sync port carries no correlation of its own.
      diag('main-result-buffered', { expected: expectedId, got: sid });
      syncBuffer.set(sid, msg);
    }
  };

  const waitForResult = (expectedId: number): unknown => {
    for (;;) {
      const buffered = syncBuffer.get(expectedId);
      if (buffered !== undefined) {
        syncBuffer.delete(expectedId);
        return buffered;
      }
      Atomics.wait(signal, 0, 0);
      Atomics.store(signal, 0, 0);
      const msg = drain(expectedId);
      if (msg !== undefined) return msg;
    }
  };

  // Replace function args with {__napi_cb} markers, keeping the function local.
  // Each registered callback takes an event-loop keep-alive ref (released when
  // the provider drops the callback), so a stored callback holds the process
  // open like an in-process `ThreadsafeFunction` would.
  const encodeArgs = (args: unknown[]): unknown[] =>
    args.map((a) => {
      if (typeof a !== 'function') return a;
      const handle = nextHandle++;
      callbacks.set(handle, a as Callback);
      refAsync();
      return { __napi_cb: handle };
    });

  // Block until the worker has connected/launched and handshaked. The ready /
  // init-error message is tagged with the reserved sync id 0.
  let nextSyncId = 1;
  const ready = waitForResult(0) as { ok: false; error: string } | { ready: true } | undefined;
  if (ready && 'ok' in ready && !ready.ok) {
    worker.terminate();
    throw new Error(ready.error);
  }

  return {
    call(fn, args) {
      if (closed) throw new Error('provider is closed');
      const syncId = nextSyncId++;
      diag('main-sync-call', { syncId, fn });
      port1.postMessage({ syncId, fn, args: encodeArgs(args) });
      const msg = waitForResult(syncId) as { ok: true; result: unknown } | { ok: false; error: string };
      diag('main-sync-result', { syncId, fn, ok: msg.ok });
      if (msg.ok) return msg.result;
      throw new Error(msg.error + diagTrace());
    },
    callAsync(fn, args) {
      if (closed) return Promise.reject(new Error('provider is closed'));
      const id = nextCallId++;
      refAsync();
      return new Promise((resolve, reject) => {
        pending.set(id, { resolve, reject });
        asyncMain.postMessage({ asyncCall: true, id, fn, args: encodeArgs(args) });
      });
    },
    trackExternal(value) {
      const token = externalToken(value);
      if (token !== undefined) externals.register(value as object, token);
    },
    close() {
      // Release keep-alive refs and fail outstanding work. Idempotent with
      // onProviderGone(), which may have already run if the provider died.
      closed = true;
      callbacks.clear();
      cbReorder.clear();
      refCount = 0;
      asyncMain.unref();
      for (const p of pending.values()) p.reject(new Error('provider is closed'));
      pending.clear();
      if (workerTerminated) return;
      workerTerminated = true;
      port1.postMessage({ close: true });
      // Wait for the worker to finish provider shutdown + socket cleanup before
      // terminating it, so no socket files are leaked.
      Atomics.wait(signal, 0, 0);
      worker.terminate();
    },
  };
}

/** Provider-bound class constructors keyed by class name. */
type ClassMap = Record<string, new (...args: unknown[]) => unknown>;

/** Launch a provider child and return a synchronous handle (Node is parent). */
export function launchProviderSync(options: LaunchSyncOptions): SyncProvider {
  return spawnSyncProvider('launch', options);
}

/**
 * Attach class proxies to a sync binding. Generated classes take the
 * `SyncProvider` as a trailing constructor arg; this wraps each so callers write
 * `new native.Counter(5)` and the provider is injected automatically. Returns a
 * binding that resolves class names to the bound ctors and everything else to
 * the underlying function binding.
 */
/** A factory free fn that returns a class instance: its proxy class plus whether
 *  the Rust fn is `async` (dispatched non-blocking) or sync (blocking). */
type Factory = {
  cls: { __fromHandle(p: SyncProvider, h: unknown): unknown };
  isAsync: boolean;
  /** Provider-side dispatch key (Rust fn name). Falls back to `camelToSnake`
   *  of the JS name when omitted, for factories without a `#[napi(js_name)]`. */
  rustName?: string;
};

/**
 * Attach class proxies to a binding. Generated classes take the `SyncProvider`
 * as a trailing constructor arg; this wraps each so callers write
 * `new native.Counter(5)` and the provider is injected automatically. Free
 * functions that return a class instance (`factories`) are wrapped to mint the
 * proxy: async factories dispatch non-blocking and resolve a `Promise`, sync
 * ones block. Returns a binding resolving class names to bound ctors and
 * everything else to the underlying function binding.
 */
export function bindClasses<T extends object>(
  binding: T,
  provider: SyncProvider,
  classes: ClassMap,
  factories: Record<string, Factory> = {}
): T {
  const bound: Record<string, unknown> = {};
  for (const [name, Ctor] of Object.entries(classes)) {
    bound[name] = class extends (Ctor as new (...a: unknown[]) => object) {
      constructor(...args: unknown[]) {
        super(...args, provider);
      }
    };
  }
  for (const [name, { cls, isAsync, rustName }] of Object.entries(factories)) {
    const wireName = rustName ?? camelToSnake(name);
    bound[name] = isAsync
      ? (...args: unknown[]) =>
          provider.callAsync(wireName, args).then((h) => cls.__fromHandle(provider, h))
      : (...args: unknown[]) => cls.__fromHandle(provider, provider.call(wireName, args));
  }
  return new Proxy(binding, {
    get: (t, p) =>
      typeof p === 'string' && p in bound ? bound[p] : (t as Record<PropertyKey, unknown>)[p],
  });
}

/** Connect synchronously as a child, using the `NAPI_OOP_SOCKET` env var. */
export function connectFromEnvSync(): SyncProvider {
  const provider = spawnSyncProvider('connectEnv', { command: '' });
  // The worker has already snapshotted the environment at construction (and it
  // reads the socket path from `SOCKET_ENV` there). Clear the token from this
  // thread now that the handoff is done, so any child process this one later
  // spawns doesn't inherit the one-shot socket and hang dialing the parent.
  // Mirrors `connectFromEnv`.
  delete process.env[SOCKET_ENV];
  return provider;
}

/**
 * Wrap a [`SyncProvider`] as a typed object that mirrors native semantics: sync
 * Rust fns block and return their value; `async` Rust fns (named in `asyncFns`)
 * dispatch without blocking the event loop and resolve a `Promise`. Functions
 * named in `externalFns` return an External handle that is registered for
 * GC-driven release of the provider-side slab.
 */
export function createSyncBinding<T extends object>(
  provider: SyncProvider,
  asyncFns: readonly string[] = [],
  externalFns: readonly string[] = [],
  wireNames: Readonly<Record<string, string>> = {}
): T {
  const asyncSet = new Set(asyncFns);
  const externalSet = new Set(externalFns);
  const cache = new Map<string, (...args: unknown[]) => unknown>();
  return new Proxy({} as T, {
    get(_target, property) {
      if (typeof property !== 'string') return undefined;
      let fn = cache.get(property);
      if (!fn) {
        // The wire name is the provider-side dispatch key (the Rust function
        // name). It is `camelToSnake(jsName)` for the common case, but a fn
        // declared with `#[napi(js_name = "…")]` has a JS name that is *not* the
        // camelCase of its Rust name, so the manifest-derived map takes priority.
        const wireName = wireNames[property] ?? camelToSnake(property);
        const isAsync = asyncSet.has(property);
        const tracks = externalSet.has(property);
        fn = isAsync
          ? (...args: unknown[]) => {
              const p = provider.callAsync(wireName, args);
              return tracks ? p.then((r) => (provider.trackExternal(r), r)) : p;
            }
          : (...args: unknown[]) => {
              const r = provider.call(wireName, args);
              if (tracks) provider.trackExternal(r);
              return r;
            };
        cache.set(property, fn);
      }
      return fn;
    },
  });
}
