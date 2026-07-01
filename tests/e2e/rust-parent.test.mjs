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
  assert.equal(r.greetNone, 'hello, world', 'undefined Option<String> arg decodes as None');
  assert.equal(r.greetSome, 'hello, Bert', 'present Option<String> arg decodes as Some');
  assert.equal(r.scaleOmitted, 7, 'omitted trailing Option arg decodes as None');
  assert.equal(r.scaleGiven, 21, 'present trailing Option arg decodes as Some');
  assert.equal(r.constAnswer, 42, '#[napi] const is exposed as its concrete value, not a stub');
  assert.equal(
    r.echoedConst,
    42,
    'a #[napi] const passed as an Option<f64> arg decodes as a number (not a callback map)'
  );
  assert.deepEqual(r.multiply, [42, 72]);
  assert.equal(r.timerFiredDuringCall, true, 'event loop stays free during async calls');
  assert.equal(r.sum, 60);
  assert.deepEqual(r.sumSteps, [10, 30, 60]);
  assert.equal(r.tsfnSum, 60);
  assert.deepEqual(r.reversed, [4, 3, 2, 1]);

  // Synchronous-callback reentrancy round-trips identically when Rust is the
  // parent: the outer call and the reentrant call each keep their own result.
  assert.equal(r.reentrantOuter, 111, 'outer sync call returns its own result under callback reentrancy');
  assert.equal(r.reentrantCbResult, 222, 'reentrant sync call in a callback returns its own result');
  assert.equal(r.big, '42');
  assert.equal(r.bigEcho, '123456789012345678901234567890', 'wide BigInt round-trips with full precision');
  assert.equal(r.bigEchoNeg, '-98765432109876543210987654321', 'negative wide BigInt round-trips');
  assert.equal(r.pointLabel, 'p', 'object field exposed camelCased');
  assert.equal(r.pointDesc, 'p=(2,3)', 'object decoded back by value');
  assert.equal(r.imageArea, 20, '&External<T> derefs to inner value');

  // Nested Option<#[napi(object)]> success variant round-trips identically when
  // Rust is the parent: truthy `.input` with fields intact, nil `.errorResult`.
  assert.equal(r.preparedHasInput, true, 'nested Option<object> Some decodes as a truthy object');
  assert.equal(r.preparedShellId, 'e2e-shell', 'nested object string field intact');
  assert.equal(r.preparedDelay, 1, 'nested object integral f64 field intact');
  assert.equal(r.preparedErrorNull, true, 'sibling Option<String> None decodes as nil');

  // A `None` Option<String> field must decode as `undefined` with its key
  // omitted — identical to in-proc napi-derive — never as `null`. Guards a real
  // CLI bug where `scope: null` over the wire was rejected by a strict
  // `scope === undefined` switch as an "invalid scope".
  assert.equal(r.scopeNoneIsUndefined, true, 'None Option<String> field is strictly undefined, not null');
  assert.equal(r.scopeNoneKeyAbsent, true, 'None Option<String> field key is omitted from the object');
  assert.equal(r.scopeSomeValue, 'siblings', 'Some Option<String> field carries its value');


  assert.equal(r.counter, 7);

  // Class round-trips identically when Rust is the parent.
  assert.equal(r.afterAdd, 8);
  assert.equal(r.value, 8);
  assert.equal(r.childValue, 108);
  assert.equal(r.parentUnchanged, 8);
  assert.equal(r.madeValue, 40);
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
