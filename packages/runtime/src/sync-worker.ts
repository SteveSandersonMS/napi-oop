// Worker thread backing the provider handle.
//
// All socket I/O lives here. The worker launches (or connects to) the provider
// and owns the async `Peer`. Two channels reach the main thread:
//
//  - `port` (sync): the main thread blocks on `Atomics.wait` for each sync call;
//    the worker performs the call, posts the result, then bumps + notifies the
//    signal. Callbacks fired *while a sync call is in flight* go here too, so the
//    main thread drains and fires them synchronously before the call returns.
//  - `asyncPort` (async): non-blocking `async` calls, their results, External
//    releases, and callbacks fired while the main thread is idle. Delivered on
//    the main thread's event loop, so they never block it.

import { workerData, MessagePort } from 'worker_threads';

import { connectFromEnv, connectPath, launchProvider } from './index';
import { diag, setDiagRole } from './diag';
import type { Peer } from './peer';

interface InitData {
  signal: Int32Array;
  port: MessagePort;
  asyncPort: MessagePort;
  mode: 'launch' | 'connectEnv' | 'connectPath';
  command?: string;
  args?: string[];
  socketPath?: string;
}

interface SyncRequest {
  syncId: number;
  fn: string;
  args: unknown[];
}

interface AsyncRequest {
  asyncCall: true;
  id: number;
  fn: string;
  args: unknown[];
}

interface ReleaseRequest {
  release: true;
  token: number;
}

const { signal, port, asyncPort, mode, command, args, socketPath } = workerData as InitData;
setDiagRole('worker');

let close: (() => void | Promise<void>) | undefined;

function wake(): void {
  Atomics.store(signal, 0, 1);
  Atomics.notify(signal, 0);
}

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** True if `v` is a `{ __napi_cb: number }` callback marker. */
function callbackHandle(v: unknown): number | undefined {
  if (v && typeof v === 'object' && '__napi_cb' in v) {
    const h = (v as { __napi_cb: unknown }).__napi_cb;
    return typeof h === 'number' ? h : undefined;
  }
  return undefined;
}

async function init(): Promise<Peer> {
  if (mode === 'launch') {
    const provider = await launchProvider({ command: command!, args, socketPath });
    close = provider.close;
    return provider.peer;
  }
  if (mode === 'connectPath') {
    return connectPath(socketPath!);
  }
  return connectFromEnv();
}

void init().then(
  (peer) => {
    // Signal that the worker is ready before handling any calls. The ready
    // message is tagged with the reserved sync id 0.
    port.postMessage({ ready: true, syncId: 0 });
    wake();

    // The number of blocking sync calls currently being serviced — i.e. how many
    // times the main thread is parked in `Atomics.wait`. It exceeds one when a
    // synchronous callback reenters with another sync call. Callbacks fired while
    // any sync call is in flight are routed over the sync port so they are
    // drained synchronously; otherwise the main thread is in its event loop and
    // callbacks go over the async port.
    let syncInFlight = 0;

    const installCallback = (handle: number): void => {
      peer.registerCallback(handle, (...cbArgs: unknown[]) => {
        if (syncInFlight > 0) {
          diag('worker-cb-sync', { handle, syncInFlight });
          port.postMessage({ cb: true, handle, args: cbArgs });
          wake();
        } else {
          diag('worker-cb-async', { handle });
          asyncPort.postMessage({ cb: true, handle, args: cbArgs });
        }
      });
    };

    // When the provider drops a callback (its last `ThreadsafeFunction` clone
    // released), tell the main thread so it can drop its registry entry and
    // release the event-loop keep-alive ref it took when the callback was sent.
    peer.onCallbackReleased = (handle: number): void => {
      asyncPort.postMessage({ cbRelease: true, handle });
    };

    // When the provider connection drops, tell the main thread so it releases
    // every callback keep-alive ref — a dead provider can never fire those
    // callbacks again, so they must not hold the caller's event loop open.
    peer.onDisconnect = (): void => {
      asyncPort.postMessage({ providerClosed: true });
    };

    const installCallbacks = (callArgs: unknown[]): void => {
      for (const a of callArgs) {
        const handle = callbackHandle(a);
        if (handle !== undefined) installCallback(handle);
      }
    };

    port.on('message', (msg: SyncRequest | { close: true }) => {
      if ('close' in msg) {
        Promise.resolve(close?.())
          .then(() => peer.close())
          .finally(wake);
        return;
      }
      installCallbacks(msg.args);
      syncInFlight += 1;
      diag('worker-sync-call', { syncId: msg.syncId, fn: msg.fn, syncInFlight });
      peer.call(msg.fn, msg.args).then(
        (result) => {
          syncInFlight -= 1;
          diag('worker-sync-result', { syncId: msg.syncId, fn: msg.fn, ok: true });
          port.postMessage({ syncId: msg.syncId, ok: true, result });
          wake();
        },
        (err: unknown) => {
          syncInFlight -= 1;
          diag('worker-sync-result', { syncId: msg.syncId, fn: msg.fn, ok: false });
          port.postMessage({ syncId: msg.syncId, ok: false, error: errorMessage(err) });
          wake();
        }
      );
    });

    asyncPort.on('message', (msg: AsyncRequest | ReleaseRequest) => {
      if ('release' in msg) {
        peer.releaseExternal(msg.token);
        return;
      }
      installCallbacks(msg.args);
      diag('worker-async-call', { id: msg.id, fn: msg.fn });
      peer.call(msg.fn, msg.args).then(
        (result) => {
          diag('worker-async-result', { id: msg.id, fn: msg.fn, ok: true });
          asyncPort.postMessage({ asyncResult: true, id: msg.id, ok: true, result });
        },
        (err: unknown) => {
          diag('worker-async-result', { id: msg.id, fn: msg.fn, ok: false });
          asyncPort.postMessage({
            asyncResult: true,
            id: msg.id,
            ok: false,
            error: errorMessage(err),
          });
        }
      );
    });
  },
  (err: unknown) => {
    port.postMessage({ ok: false, error: errorMessage(err), syncId: 0 });
    wake();
  }
);
