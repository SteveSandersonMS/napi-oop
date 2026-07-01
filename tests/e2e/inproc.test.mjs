// E2E: the SAME dual-ABI cdylib loaded **in-process** by Node as a traditional
// napi addon (`require('./fixture.node')`), with no provider, no socket, no child
// process. This is the third hosting mode — the napi door of the artifact whose
// napi-oop door the out-of-process tests drive. It proves a single build output
// serves both: every flow here runs through real N-API in Node's own process.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const native = require('./fixture.node');

/** Poll until `pred()` holds (or a timeout elapses), yielding to libuv each turn
 *  so the real napi ThreadsafeFunction queue can drain — just as a consumer of a
 *  traditional napi TSFN would await its asynchronous, fire-and-forget delivery. */
async function until(pred, timeoutMs = 2000) {
  const start = Date.now();
  while (!pred() && Date.now() - start < timeoutMs) {
    await new Promise((r) => setTimeout(r, 5));
  }
}

test('in-proc: every flow through the real napi addon door', async () => {
  // Sync fn + Option<String> (None vs Some) + trailing Option<T> omitted.
  assert.equal(native.addNumbers(2, 3), 5);
  assert.equal(native.greet(undefined), 'hello, world');
  assert.equal(native.greet('Bert'), 'hello, Bert');
  assert.equal(native.scale(7), 7);
  assert.equal(native.scale(7, 3), 21);

  // async Rust fn surfaces as a Promise (never blocks), and overlaps concurrently.
  let timerFiredDuringCall = false;
  const timer = new Promise((r) =>
    setTimeout(() => {
      timerFiredDuringCall = true;
      r();
    }, 30)
  );
  const t0 = Date.now();
  const [p, q] = await Promise.all([native.multiplySlow(6, 7), native.multiplySlow(8, 9)]);
  const concurrentMs = Date.now() - t0;
  await timer;
  assert.deepEqual([p, q], [42, 72]);
  assert.ok(concurrentMs < 350, `expected concurrent (<350ms), got ${concurrentMs}ms`);
  assert.equal(timerFiredDuringCall, true, 'async call never blocks the event loop');

  // Both callback forms. The `impl Fn(T)` sugar maps to a synchronous napi
  // `Function`: like traditional napi, the callback fires inline on the JS thread
  // during the sync call, so its steps are populated by the time the call returns
  // — no event-loop turn needed. The explicit `ThreadsafeFunction<T>` is a real
  // napi TSFN: its NonBlocking calls queue onto the event loop (fire-and-forget),
  // so they arrive on the next turn, exactly as a traditional napi TSFN does.
  const sumSteps = [];
  const sum = native.sumEach([10, 20, 30], (running) => sumSteps.push(running));
  assert.equal(sum, 60);
  assert.deepEqual(sumSteps, [10, 30, 60], 'impl Fn callback fires synchronously (sync Function)');

  const tsfnSteps = [];
  // A real napi `ThreadsafeFunction` is `CalleeHandled` by default, so its JS
  // callback receives `(err, value)` — the vanilla napi convention.
  const tsfnSum = native.sumEachTsfn([10, 20, 30], (_err, running) => tsfnSteps.push(running));
  assert.equal(tsfnSum, 60);
  await until(() => tsfnSteps.length === 3);
  assert.deepEqual(tsfnSteps, [10, 30, 60], 'ThreadsafeFunction callbacks drain on the event loop');

  // Buffer + BigInt round-trips.
  assert.deepEqual(Array.from(native.reverseBytes(Buffer.from([1, 2, 3, 4]))), [4, 3, 2, 1]);
  assert.equal(native.doubleBig(21n).toString(), '42');
  // Arbitrary-precision BigInt: wider than 64 bits and negative, echoed unchanged.
  assert.equal(
    native.echoBig(123456789012345678901234567890n).toString(),
    '123456789012345678901234567890',
    'wide BigInt round-trips with full precision'
  );
  assert.equal(
    native.echoBig(-98765432109876543210987654321n).toString(),
    '-98765432109876543210987654321',
    'negative wide BigInt round-trips'
  );

  // #[napi(object)] value struct: camelCased field + by-value arg back in.
  const point = native.makePoint(2, 3, 'p');
  assert.equal(point.labelText, 'p');
  assert.equal(native.describePoint(point), 'p=(2,3)');

  // External<T>: minted handle read back, and a &External<T> param via Deref.
  const image = native.imageMake(4, 5);
  assert.equal(native.imageArea(image), 20);
  assert.equal(native.readCounter(native.makeCounter(7)), 7);

  // Nested Option<#[napi(object)]> success variant through the real napi door:
  // truthy `.input` with fields intact, nil `.errorResult`.
  const prepared = native.prepareShell('{"delay":1,"shellId":"e2e-shell"}');
  assert.ok(prepared.input, 'nested Option<object> Some is a truthy object');
  assert.equal(prepared.input.shellId, 'e2e-shell', 'nested object string field intact');
  assert.equal(prepared.input.delay, 1, 'nested object integral f64 field intact');
  assert.equal(prepared.errorResult ?? null, null, 'sibling Option<String> None is nil');

  // Class: sync ctor + async unsafe mutate + sync getter + async cross-method.
  const obj = new native.Counter(5);
  assert.equal(await obj.addSlow(3), 8);
  assert.equal(obj.value, 8);
  const child = await obj.forkSlow(100);
  assert.equal(child.value, 108);
  assert.equal(obj.value, 8, 'parent unaffected by child');

  // Free-fn factory returns a class; non-Clone class by-move (factory + cross-class).
  assert.equal(native.makeCounterClass(40).value, 40);
  assert.equal(native.makeTally(11).total, 11);
  assert.equal(obj.snapshot().total, 8);

  // js_name divergence at class, method, and free-fn level.
  const renamed = new native.RenamedBox(12);
  assert.equal(renamed.bump(5), 17);
  assert.equal(renamed.duplicate().value, 17);
  assert.equal(native.makeBertBox(30).bump(2), 32);
  assert.equal(native.bertShout('hi'), 'HI');
  assert.equal(obj.bertReset(), 0);
});
