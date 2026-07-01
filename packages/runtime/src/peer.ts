// A connected peer: the caller side of the boundary. Sends `Request`s over the
// framed MessagePack transport and resolves the matching `Response`/`Error` by
// correlation id.

import type { Socket } from 'net';

import { createFrameDecoder, encodeFrame } from './framing';
import {
  CallbackRef,
  EXTERNAL_KEY,
  Hello,
  Message,
  PROTOCOL_VERSION,
  Request,
  Role,
} from './messages';

type Callback = (...args: unknown[]) => unknown;

interface Pending {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
}

/** A connected peer that can call into the out-of-process Rust side. */
export class Peer {
  private nextId = 1;
  private readonly pending = new Map<number, Pending>();
  /** JS functions passed as args, kept alive so the provider can invoke them. */
  private readonly callbacks = new Map<number, Callback>();
  /**
   * Awaitable variants of held JS callbacks, invoked by the provider via
   * `callbackCall` (request/response). Distinct from `callbacks` so the
   * fire-and-forget `callbackInvoke` fast path is unaffected. In the direct
   * binding this stays empty and `callbackCall` falls back to `callbacks`; the
   * worker-backed `SyncProvider` installs an awaitable proxy here that forwards
   * to the main thread and returns a `Promise` for its result.
   */
  private readonly callbackCallHandlers = new Map<number, Callback>();
  private nextHandle = 1;
  private closed = false;
  /**
   * Notified when the provider releases a callback handle (its last
   * `ThreadsafeFunction` clone dropped). The worker-backed `SyncProvider` uses
   * this to forward the release to the main thread, which keeps the process
   * event loop alive while a callback is live — mirroring how an in-process
   * `ThreadsafeFunction` is ref'd by default until dropped.
   */
  onCallbackReleased?: (handle: number) => void;
  /**
   * Notified once when the underlying socket closes or errors. The worker-backed
   * `SyncProvider` uses this to release any callback keep-alive refs so a dead
   * provider can't hold the caller's event loop open forever.
   */
  onDisconnect?: () => void;
  /** Releases provider-side External slab entries when the JS handle is GC'd. */
  private readonly externals = new FinalizationRegistry<number>((token) => {
    if (!this.closed) this.socket.write(encodeFrame({ type: 'releaseExternal', token }));
  });

  /** Register a returned External marker so its slab entry is freed on GC. */
  trackExternal(value: unknown): void {
    const token = externalToken(value);
    if (token !== undefined) this.externals.register(value as object, token);
  }

  /** Free a provider-side External slab entry by token (fire-and-forget). Used
   *  by the worker-backed provider handle, whose main-thread `FinalizationRegistry`
   *  detects the GC and asks the worker to release. */
  releaseExternal(token: number): void {
    if (!this.closed) this.socket.write(encodeFrame({ type: 'releaseExternal', token }));
  }

  private constructor(
    private readonly socket: Socket,
    /** The peer's advertised `Hello` (role + the functions it exposes). */
    readonly remote: Hello
  ) {
    const decode = createFrameDecoder((msg) => this.onMessage(msg as Message));
    socket.on('data', decode);
    socket.on('close', () => this.handleDisconnect(new Error('peer connection closed')));
    socket.on('error', (err) => this.handleDisconnect(err));
  }

  /**
   * Handle the socket closing or erroring. Marks the peer closed *before*
   * failing in-flight calls so any subsequent `call()` rejects immediately
   * rather than writing to a dead socket and never resolving — which, on the
   * synchronous (worker-backed) path, would park the caller's main thread in
   * `Atomics.wait` forever. Fires `onDisconnect` once so keep-alive refs are
   * released.
   */
  private handleDisconnect(error: Error): void {
    if (this.closed) return;
    this.closed = true;
    this.failAll(error);
    this.onDisconnect?.();
  }

  /**
   * Perform the caller handshake over an already-connected socket: send our
   * `Hello`, await the peer's, and verify the protocol versions match.
   */
  static handshake(socket: Socket, role: Role = 'caller'): Promise<Peer> {
    return new Promise((resolve, reject) => {
      const decode = createFrameDecoder((msg) => {
        const hello = msg as Message;
        if (hello.type !== 'hello') {
          reject(new Error(`expected hello during handshake, got ${hello.type}`));
          return;
        }
        if (hello.version !== PROTOCOL_VERSION) {
          reject(
            new Error(
              `protocol version mismatch: local ${PROTOCOL_VERSION}, peer ${hello.version}`
            )
          );
          return;
        }
        // Hand the socket off to a Peer; remove this bootstrap listener first so
        // it doesn't compete with the Peer's own data handler.
        socket.removeListener('data', decode);
        resolve(new Peer(socket, hello));
      });
      socket.on('data', decode);
      socket.on('error', reject);
      socket.write(
        encodeFrame({
          type: 'hello',
          version: PROTOCOL_VERSION,
          role,
          functions: [],
        } satisfies Hello)
      );
    });
  }

  /** Call a function exposed by the peer, resolving with its return value. */
  call(fn: string, args: unknown[]): Promise<unknown> {
    if (this.closed) {
      return Promise.reject(new Error('peer is closed'));
    }
    const id = this.nextId++;
    const request: Request = { type: 'request', id, fn, args: args.map((a) => this.encodeArg(a)) };
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.socket.write(encodeFrame(request));
    });
  }

  /** Replace a JS function arg with a callback handle marker; pass others as-is. */
  private encodeArg(arg: unknown): unknown {
    if (typeof arg !== 'function') return arg;
    const handle = this.nextHandle++;
    this.callbacks.set(handle, arg as Callback);
    return { __napi_cb: handle } satisfies CallbackRef;
  }

  /**
   * Register a callback under a caller-assigned handle. Used by the sync binding,
   * where the main thread allocates handles and the worker installs a proxy that
   * forwards each provider invocation back to the main thread (fire-and-forget).
   */
  registerCallback(handle: number, fn: Callback): void {
    this.callbacks.set(handle, fn);
  }

  /**
   * Register the **awaitable** variant of a callback under a caller-assigned
   * handle, invoked by the provider via `callbackCall`. Used by the sync binding,
   * where the worker installs a proxy that forwards to the main thread and
   * returns a `Promise` resolving to the callback's result. In the direct binding
   * this is unused (`callbackCall` falls back to the plain `callbacks` map).
   */
  registerCallbackCall(handle: number, fn: Callback): void {
    this.callbackCallHandlers.set(handle, fn);
  }

  /** Close the connection and reject any in-flight calls. */
  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.callbacks.clear();
    this.callbackCallHandlers.clear();
    this.socket.end();
    this.failAll(new Error('peer closed'));
  }

  private onMessage(msg: Message): void {
    if (msg.type === 'callbackInvoke') {
      this.handleCallback(msg.handle, msg.args);
      return;
    }
    if (msg.type === 'callbackCall') {
      this.handleCallbackCall(msg.callId, msg.handle, msg.args);
      return;
    }
    if (msg.type === 'release') {
      this.callbacks.delete(msg.handle);
      this.callbackCallHandlers.delete(msg.handle);
      this.onCallbackReleased?.(msg.handle);
      return;
    }
    if (msg.type !== 'response' && msg.type !== 'error') return;
    const pending = this.pending.get(msg.id);
    if (!pending) return;
    this.pending.delete(msg.id);
    if (msg.type === 'response') {
      pending.resolve(msg.result);
    } else {
      pending.reject(new Error(msg.message));
    }
  }

  /** Run a JS callback the provider fired. Fire-and-forget: no reply is sent. */
  private handleCallback(handle: number, args: unknown[]): void {
    const cb = this.callbacks.get(handle);
    if (!cb) return;
    try {
      cb(...args);
    } catch {
      // Fire-and-forget: callback errors are the caller's concern, not the wire's.
    }
  }

  /**
   * Run a JS callback the provider invoked via `callbackCall` and reply with its
   * result (request/response), mirroring napi's `ThreadsafeFunction::call_async`.
   * The callback may return a value or a `Promise`; either way the resolved value
   * is sent back as a `callbackResult`, or a rejection/throw as a `callbackError`.
   */
  private handleCallbackCall(callId: number, handle: number, args: unknown[]): void {
    const cb = this.callbackCallHandlers.get(handle) ?? this.callbacks.get(handle);
    if (!cb) {
      this.sendCallbackError(callId, `no callback registered for handle ${handle}`);
      return;
    }
    Promise.resolve()
      .then(() => cb(...args))
      .then(
        (result) => this.sendCallbackResult(callId, result),
        (err: unknown) => this.sendCallbackError(callId, err instanceof Error ? err.message : String(err))
      );
  }

  private sendCallbackResult(callId: number, result: unknown): void {
    if (!this.closed) this.socket.write(encodeFrame({ type: 'callbackResult', callId, result }));
  }

  private sendCallbackError(callId: number, message: string): void {
    if (!this.closed) this.socket.write(encodeFrame({ type: 'callbackError', callId, message }));
  }

  private failAll(error: Error): void {
    for (const pending of this.pending.values()) {
      pending.reject(error);
    }
    this.pending.clear();
  }
}

/** Return the token if `v` is a top-level `{ __napi_ext }` marker, else undefined. */
function externalToken(v: unknown): number | undefined {
  if (v && typeof v === 'object' && EXTERNAL_KEY in v) {
    const t = (v as Record<string, unknown>)[EXTERNAL_KEY];
    return typeof t === 'number' ? t : undefined;
  }
  return undefined;
}
