// E2E: Node is the parent, spawning the Rust provider as a child. Exercises
// every supported type and callback flow across the real socket boundary.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { run, parseResult } from './run.mjs';

const here = new URL('.', import.meta.url);

test('node-parent: every flow over the real socket boundary', async () => {
  const { code, out } = await run('node', ['dist/run-all.js'], here);
  assert.equal(code, 0, out);
  const r = parseResult(out);

  assert.equal(r.role, 'node-parent');
  assert.equal(r.add, 5);
  assert.deepEqual(r.multiply, [42, 72]);
  assert.ok(r.concurrentMs < 350, `expected concurrent (<350ms), got ${r.concurrentMs}ms`);
  assert.equal(r.timerFiredDuringCall, true, 'event loop stays free during async calls');
  assert.equal(r.sum, 60);
  assert.deepEqual(r.sumSteps, [10, 30, 60]);
  assert.equal(r.tsfnSum, 60);
  assert.deepEqual(r.tsfnSteps, [10, 30, 60]);
  assert.deepEqual(r.reversed, [4, 3, 2, 1]);
  assert.equal(r.big, '42');
  assert.equal(r.counter, 7);

  // Class: sync ctor + async mutate + sync getter + async cross-method return.
  assert.equal(r.afterAdd, 8, 'async add mutates provider-side state');
  assert.equal(r.value, 8, 'sync getter reads mutated state');
  assert.equal(r.childValue, 108, 'forkSlow snapshots parent+by');
  assert.equal(r.parentUnchanged, 8, 'parent unaffected by child');
  assert.equal(r.madeValue, 40, 'free-fn factory returns a working class instance');
});
