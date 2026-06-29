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
    // Signal that the worker is ready before handling any calls.
    port.postMessage({ ready: true });
    wake();

    // True exactly while a blocking sync call is being serviced — i.e. while the
    // main thread is parked in `Atomics.wait`. Callbacks fired in this window are
    // routed over the sync port so they are drained synchronously; otherwise the
    // main thread is in its event loop and callbacks go over the async port.
    let syncInFlight = false;

    const installCallback = (handle: number): void => {
      peer.registerCallback(handle, (...cbArgs: unknown[]) => {
        if (syncInFlight) {
          port.postMessage({ cb: true, handle, args: cbArgs });
          wake();
        } else {
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
      syncInFlight = true;
      peer.call(msg.fn, msg.args).then(
        (result) => {
          syncInFlight = false;
          port.postMessage({ ok: true, result });
          wake();
        },
        (err: unknown) => {
          syncInFlight = false;
          port.postMessage({ ok: false, error: errorMessage(err) });
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
      peer.call(msg.fn, msg.args).then(
        (result) => asyncPort.postMessage({ asyncResult: true, id: msg.id, ok: true, result }),
        (err: unknown) =>
          asyncPort.postMessage({
            asyncResult: true,
            id: msg.id,
            ok: false,
            error: errorMessage(err),
          })
      );
    });
  },
  (err: unknown) => {
    port.postMessage({ ok: false, error: errorMessage(err) });
    wake();
  }
);
