// Wire message shapes, mirroring the Rust `codec::Message` enum. The enum is
// internally tagged on `type` (camelCase), so each message is a flat map like
// `{ type: 'request', id, fn, args }`.

/** Wire protocol version, kept in sync with the Rust `PROTOCOL_VERSION`. */
export const PROTOCOL_VERSION = 1;

/** Which side a peer plays. The transport itself is symmetric. */
export type Role = 'provider' | 'caller';

export interface Hello {
  type: 'hello';
  version: number;
  role: Role;
  functions: string[];
}

export interface Request {
  type: 'request';
  id: number;
  fn: string;
  args: unknown[];
}

export interface Response {
  type: 'response';
  id: number;
  result: unknown;
}

export interface ErrorMsg {
  type: 'error';
  id: number;
  message: string;
}

/** Provider asks the caller to invoke a JS callback it holds by handle id. */
export interface CallbackInvoke {
  type: 'callbackInvoke';
  id: number;
  handle: number;
  args: unknown[];
}

/** The caller's reply to a {@link CallbackInvoke}. */
export interface CallbackResult {
  type: 'callbackResult';
  id: number;
  result: unknown;
}

/** Releases a callback handle so the holder can drop it. */
export interface Release {
  type: 'release';
  handle: number;
}

/** Any message that may arrive from the peer. */
export type Message = Hello | Request | Response | ErrorMsg | CallbackInvoke | CallbackResult | Release;

/** Wire marker replacing a JS function arg: `{ __napi_cb: <handle id> }`. */
export interface CallbackRef {
  __napi_cb: number;
}
