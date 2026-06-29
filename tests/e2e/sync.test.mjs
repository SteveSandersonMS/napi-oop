// E2E: synchronous bindings. The main thread blocks for sync results while a
// worker owns the socket; an async Rust fn stays a Promise; sync callbacks fire
// between blocking calls.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { run, parseResult } from './run.mjs';

const here = new URL('.', import.meta.url);

test('sync bindings: blocking call, awaited async, deferred callbacks', async () => {
  const { code, out } = await run('node', ['dist/run-sync.js'], here);
  assert.equal(code, 0, out);
  const r = parseResult(out);

  assert.equal(r.role, 'node-parent:sync');
  assert.equal(r.add, 5);
  assert.equal(r.product, 42);
  assert.equal(r.sum, 60);
  assert.deepEqual(r.steps, [10, 30, 60]);
});
