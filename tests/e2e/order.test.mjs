// E2E: cross-port callback ordering. The sync binding splits callback delivery
// across two channels — the sync port (drained under `Atomics.wait` while a
// blocking call is in flight) and the async port (the event loop). A callback
// fired during a blocking call must NOT leapfrog an earlier one still queued on
// the async port: callbacks must be observed in the exact order the provider
// fired them, matching an in-process `ThreadsafeFunction`'s single FIFO queue.
//
// The driver arms callback "A" on the async port, then fires "B" over the sync
// port during a blocking call. A correct transport always delivers [A, B]; the
// pre-fix binding delivered [B, A] on every iteration. See ts/run-order.ts.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';

const here = new URL('.', import.meta.url);

function runDriver(iterations) {
  return new Promise((resolve, reject) => {
    const child = spawn('node', ['dist/run-order.js', String(iterations)], { cwd: here });
    let out = '';
    child.stdout.on('data', (d) => (out += d));
    child.stderr.on('data', (d) => (out += d));
    const timer = setTimeout(() => {
      child.kill();
      reject(new Error(`timed out; output so far:\n${out}`));
    }, 60000);
    child.on('error', reject);
    child.on('exit', (code) => {
      clearTimeout(timer);
      resolve({ code, out });
    });
  });
}

test('callbacks are delivered in fire order across the sync and async ports', async () => {
  // The pre-fix bug inverted *every* iteration (100% [B, A]), so a modest count
  // detects any regression with certainty. Each iteration carries ~100ms of fixed
  // waits (spin/block/settle) that don't shrink on faster hardware, so keep the
  // count low enough to stay well under the timeout on slow macOS runners.
  const iterations = 80;
  const { code, out } = await runDriver(iterations);
  const line = out.split(/\r?\n/).find((l) => l.startsWith('RESULT '));
  assert.ok(line, `no RESULT line in output:\n${out}`);
  const result = JSON.parse(line.slice('RESULT '.length));

  // Every iteration must deliver both callbacks (none dropped or coalesced)...
  assert.equal(result.complete, iterations, `some iterations lost a callback:\n${out}`);
  // ...and never out of order. Pre-fix this was `iterations` (100% inverted).
  assert.equal(result.inversions, 0, `observed out-of-order callback delivery:\n${out}`);
  assert.equal(code, 0, `driver exited non-zero:\n${out}`);
});
