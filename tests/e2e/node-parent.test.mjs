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
  assert.equal(r.greetNone, 'hello, world', 'undefined Option<String> arg decodes as None');
  assert.equal(r.greetSome, 'hello, Bert', 'present Option<String> arg decodes as Some');
  assert.deepEqual(r.multiply, [42, 72]);
  assert.ok(r.concurrentMs < 350, `expected concurrent (<350ms), got ${r.concurrentMs}ms`);
  assert.equal(r.timerFiredDuringCall, true, 'event loop stays free during async calls');
  assert.equal(r.sum, 60);
  assert.deepEqual(r.sumSteps, [10, 30, 60]);
  assert.equal(r.tsfnSum, 60);
  assert.deepEqual(r.tsfnSteps, [10, 30, 60]);
  assert.deepEqual(r.reversed, [4, 3, 2, 1]);
  assert.equal(r.big, '42');

  // #[napi(object)] value struct: camelCase field access + by-value arg.
  assert.equal(r.pointLabel, 'p', 'object field exposed camelCased');
  assert.equal(r.pointDesc, 'p=(2,3)', 'object decoded back by value');

  // External<T> read provider-side through a &External<T> param via Deref.
  assert.equal(r.imageArea, 20, '&External<T> derefs to inner value');

  assert.equal(r.counter, 7);

  // Class: sync ctor + async mutate + sync getter + async cross-method return.
  assert.equal(r.afterAdd, 8, 'async add mutates provider-side state');
  assert.equal(r.value, 8, 'sync getter reads mutated state');
  assert.equal(r.childValue, 108, 'forkSlow snapshots parent+by');
  assert.equal(r.parentUnchanged, 8, 'parent unaffected by child');
  assert.equal(r.madeValue, 40, 'free-fn factory returns a working class instance');

  // A non-Clone/non-Serialize class minted by move: as a free-fn factory return
  // and as a cross-class method return (a method on one class returning another).
  assert.equal(r.tallyTotal, 11, 'free-fn factory mints a non-Clone class by move');
  assert.equal(r.snapTotal, 8, 'cross-class method returns another class instance');
  assert.equal(r.renamedAfterBump, 17, 'renamed class ctor/method dispatch by Rust wire name');
  assert.equal(r.renamedChildValue, 17, 'renamed class method return wraps under JS class name');
  assert.equal(r.renamedMadeAfterBump, 32, 'free-fn factory returns renamed class instance');

  // #[napi(js_name = "…")]: JS names that diverge from the Rust names must still
  // reach the right provider fn (dispatched by rust_name, not camelToSnake).
  assert.equal(r.shout, 'HI', 'free fn with js_name dispatches by rust_name');
  assert.equal(r.reset, 0, 'method with js_name dispatches by rust_name');
});
