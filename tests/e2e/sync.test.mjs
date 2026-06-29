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

  // Class state lives provider-side; fork() yields an independent instance.
  assert.equal(r.afterAdd, 8, 'add() mutates provider-side state');
  assert.equal(r.value, 8, 'getter reads mutated state');
  assert.equal(r.childValue, 8, 'fork() snapshots parent value');
  assert.equal(r.childAfterAdd, 108, 'child mutates independently');
  assert.equal(r.parentUnchanged, 8, 'parent unaffected by child mutation');
});
