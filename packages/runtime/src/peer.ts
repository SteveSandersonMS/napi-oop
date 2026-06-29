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
  private nextHandle = 1;
  private closed = false;
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
    socket.on('close', () => this.failAll(new Error('peer connection closed')));
    socket.on('error', (err) => this.failAll(err));
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

  /** Close the connection and reject any in-flight calls. */
  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.callbacks.clear();
    this.socket.end();
    this.failAll(new Error('peer closed'));
  }

  private onMessage(msg: Message): void {
    if (msg.type === 'callbackInvoke') {
      this.handleCallback(msg.handle, msg.args);
      return;
    }
    if (msg.type === 'release') {
      this.callbacks.delete(msg.handle);
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
