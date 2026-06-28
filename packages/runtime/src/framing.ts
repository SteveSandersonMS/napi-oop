// Length-prefixed (u32 big-endian) framing — the Node counterpart to the Rust
// `codec` module. Each frame's payload is a MessagePack-encoded message, matching
// the Rust side's `rmp-serde` `to_vec_named` output (maps with string keys), so
// the two languages interoperate.

import { Packr } from 'msgpackr';

// `useRecords: false` keeps payloads as plain MessagePack maps (no msgpackr's
// record-extension shorthand), which is what `rmp-serde` produces and expects.
const packr = new Packr({ useRecords: false });

/** Encode a message as a length-prefixed MessagePack frame. */
export function encodeFrame(message: unknown): Buffer {
  const payload = packr.pack(message);
  const header = Buffer.allocUnsafe(4);
  header.writeUInt32BE(payload.length, 0);
  return Buffer.concat([header, payload]);
}

/**
 * Create a stateful decoder that accumulates bytes and invokes `onMessage`
 * once per complete frame. Returns a `push(chunk)` function to feed it data.
 */
export function createFrameDecoder(
  onMessage: (message: unknown) => void
): (chunk: Buffer) => void {
  let buffer = Buffer.alloc(0);
  return function push(chunk: Buffer): void {
    buffer = Buffer.concat([buffer, chunk]);
    while (buffer.length >= 4) {
      const len = buffer.readUInt32BE(0);
      if (buffer.length < 4 + len) break;
      const payload = buffer.subarray(4, 4 + len);
      buffer = buffer.subarray(4 + len);
      onMessage(packr.unpack(payload));
    }
  };
}
