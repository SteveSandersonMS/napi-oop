// Worker thread backing the synchronous calling variant.
//
// All socket I/O lives here. The worker launches (or connects to) the provider
// and owns the async `Peer`. The main thread blocks on `Atomics.wait` over a
// shared Int32Array; for each request the worker performs the async call, posts
// the structured-clone result back over a MessagePort, then bumps + notifies the
// signal so the main thread wakes and reads the result with
// `receiveMessageOnPort`. This keeps the main thread's API fully synchronous.

import { workerData, MessagePort } from 'worker_threads';

import { connectFromEnv, connectPath, launchProvider } from './index';
import type { Peer } from './peer';

interface InitData {
  signal: Int32Array;
  port: MessagePort;
  mode: 'launch' | 'connectEnv' | 'connectPath';
  command?: string;
  args?: string[];
  socketPath?: string;
}

interface CallRequest {
  fn: string;
  args: unknown[];
}

type CallResult = { ok: true; result: unknown } | { ok: false; error: string };

const { signal, port, mode, command, args, socketPath } = workerData as InitData;

let close: (() => void | Promise<void>) | undefined;

function wake(): void {
  Atomics.store(signal, 0, 1);
  Atomics.notify(signal, 0);
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

    port.on('message', (msg: CallRequest | { close: true }) => {
      if ('close' in msg) {
        Promise.resolve(close?.()).then(() => peer.close()).finally(wake);
        return;
      }
      peer.call(msg.fn, msg.args).then(
        (result) => {
          port.postMessage({ ok: true, result } satisfies CallResult);
          wake();
        },
        (err: unknown) => {
          const error = err instanceof Error ? err.message : String(err);
          port.postMessage({ ok: false, error } satisfies CallResult);
          wake();
        }
      );
    });
  },
  (err: unknown) => {
    const error = err instanceof Error ? err.message : String(err);
    port.postMessage({ ok: false, error } satisfies CallResult);
    wake();
  }
);
