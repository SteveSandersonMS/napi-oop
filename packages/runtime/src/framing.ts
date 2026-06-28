// Length-prefixed (u32 big-endian) framing — the Node counterpart to the Rust
// `codec` module. Phase 2 swaps the JSON payloads for a compact binary
// encoding; the framing (length prefix) stays the same.

/** Encode a JSON-serializable message as a length-prefixed frame. */
export function encodeFrame(message: unknown): Buffer {
  const payload = Buffer.from(JSON.stringify(message), 'utf8');
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
      onMessage(JSON.parse(payload.toString('utf8')));
    }
  };
}
