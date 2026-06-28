// @napi-oop/runtime — Node-side runtime for out-of-process napi.
//
// Connects to a Rust peer over a path-based named socket (UDS on Unix / named
// pipe on Windows via Node's `net`), never stdio. Either process may be the
// parent. This is a Phase 1 skeleton: framing is real; the peer/handshake/
// async-call surface arrives in Phases 2, 4 and 5.

export { encodeFrame, createFrameDecoder } from './framing';

/** Wire protocol version, kept in sync with the Rust `PROTOCOL_VERSION`. */
export const PROTOCOL_VERSION = 1;

/** Message kinds carried over the wire (full-duplex; either side may send). */
export type MessageType =
  | 'hello'
  | 'request'
  | 'response'
  | 'error'
  | 'callbackInvoke'
  | 'callbackResult'
  | 'release';

/**
 * A connected peer that can call into the out-of-process Rust side.
 *
 * TODO(phase4): `call(fn, args)` returning a Promise; handshake; correlation-id
 * routing; re-entrant callback handling.
 */
export interface Peer {
  call(fn: string, args: unknown[]): Promise<unknown>;
  close(): void;
}

/**
 * Connect to a Rust peer listening at `socketPath` (parent → child bootstrap is
 * added in Phase 5).
 *
 * TODO(phase4): implement using `net.connect(socketPath)` + the framing codec.
 */
export function connect(_socketPath: string): Promise<Peer> {
  throw new Error('not implemented yet (Phase 4)');
}
