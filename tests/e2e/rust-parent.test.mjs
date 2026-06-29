// E2E: Rust is the parent, spawning Node as a child that connects back over the
// socket. Same flows, opposite parentage — proves the boundary is symmetric.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { run, parseResult } from './run.mjs';

const here = new URL('.', import.meta.url);

test('rust-parent: every flow with Rust spawning Node', async () => {
  const { code, out } = await run('node', ['rust-parent.mjs'], here);
  assert.equal(code, 0, out);
  const r = parseResult(out);

  assert.equal(r.role, 'rust-parent');
  assert.equal(r.add, 5);
  assert.deepEqual(r.multiply, [42, 72]);
  assert.equal(r.timerFiredDuringCall, true, 'event loop stays free during async calls');
  assert.equal(r.sum, 60);
  assert.deepEqual(r.sumSteps, [10, 30, 60]);
  assert.equal(r.tsfnSum, 60);
  assert.deepEqual(r.reversed, [4, 3, 2, 1]);
  assert.equal(r.big, '42');
  assert.equal(r.pointLabel, 'p', 'object field exposed camelCased');
  assert.equal(r.pointDesc, 'p=(2,3)', 'object decoded back by value');
  assert.equal(r.imageArea, 20, '&External<T> derefs to inner value');
  assert.equal(r.counter, 7);

  // Class round-trips identically when Rust is the parent.
  assert.equal(r.afterAdd, 8);
  assert.equal(r.value, 8);
  assert.equal(r.childValue, 108);
  assert.equal(r.parentUnchanged, 8);
  assert.equal(r.madeValue, 40);
});
