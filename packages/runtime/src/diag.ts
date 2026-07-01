// Env-gated wire diagnostics.
//
// When `NAPI_OOP_DIAG` names a file, every `diag()` event is appended to it as a
// JSON line *and* kept in an in-memory ring buffer so the recent trace can be
// embedded into an error surfaced to the caller. That matters for out-of-process
// E2E runs where stdout from a child/worker may not reach the test log, but a
// thrown error's message does. When the env var is unset, `diag()` is a cheap
// no-op (a single boolean check) so there is zero overhead in production.
//
// Both the main thread and the socket worker (separate JS contexts, same file
// path) append here; each write is a single line under `O_APPEND`, and every
// record carries a `role` so an interleaved file is still readable.

import { appendFileSync } from 'fs';

const DIAG_FILE = process.env.NAPI_OOP_DIAG;
const ENABLED = typeof DIAG_FILE === 'string' && DIAG_FILE.length > 0;
const RING_MAX = 256;
const ring: string[] = [];

/** The label written into every record's `role` field (`main` vs `worker`). */
let role = 'main';

/** Tag this JS context's diag records (called once by the worker at startup). */
export function setDiagRole(value: string): void {
  role = value;
}

/** Whether diagnostics are enabled for this process. */
export function diagEnabled(): boolean {
  return ENABLED;
}

/** Record a wire event. No-op unless `NAPI_OOP_DIAG` is set. */
export function diag(event: string, data?: Record<string, unknown>): void {
  if (!ENABLED) return;
  const line = JSON.stringify({ ts: Date.now(), pid: process.pid, role, event, ...data });
  ring.push(line);
  if (ring.length > RING_MAX) ring.shift();
  try {
    appendFileSync(DIAG_FILE as string, line + '\n');
  } catch {
    // Diagnostics must never break the caller.
  }
}

/**
 * The recent diag events as a human-readable block, for appending to an error
 * message so the trace reaches the test log even when the diag file isn't
 * collected. Empty string when diagnostics are disabled.
 */
export function diagTrace(): string {
  if (!ENABLED || ring.length === 0) return '';
  return `\n--- napi-oop diag (last ${ring.length} events, role=${role}) ---\n${ring.join('\n')}`;
}
