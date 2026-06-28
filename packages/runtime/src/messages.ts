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

/** Any message that may arrive from the peer. */
export type Message = Hello | Request | Response | ErrorMsg;
