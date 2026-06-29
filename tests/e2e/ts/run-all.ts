// E2E driver: exercises every cross-process flow napi-oop supports through the
// single binding (a faithful native mirror: sync Rust fns/methods block for a
// value, `async` ones resolve a non-blocking Promise). Prints one
// machine-readable `RESULT <json>` line the harness asserts on. Symmetric in
// parentage: run directly (Node parent spawns Rust) or via the rust-parent
// launcher (Rust parent spawned us; NAPI_OOP_SOCKET is set).

import { join } from 'path';

import { SOCKET_ENV, connectFromEnvSync, launchProviderSync, type SyncProvider } from 'napi-oop-runtime';

import { bind } from './generated/bindings';

function providerCommand(): string {
  return join(__dirname, '..', '..', '..', 'target', 'release', 'e2e-provider');
}

async function exercise(provider: SyncProvider) {
  const native = bind(provider);

  // Sync fn: blocks and returns the value directly.
  const add = native.addNumbers(2, 3);

  // Option<String> param: `undefined` must decode provider-side as `None`
  // (wire nil), a present value as `Some`.
  const greetNone = native.greet(undefined);
  const greetSome = native.greet('Bert');

  // Trailing Option<T> omitted: the binding sends *fewer* args than the declared
  // arity, and the provider must decode the missing tail as `None` (factor=1).
  const scaleOmitted = native.scale(7);
  const scaleGiven = native.scale(7, 3);

  // Concurrency + non-blocking proof: two 200ms async calls overlap (finishing
  // well under 400ms), and a 30ms timer fires *while they are in flight* — which
  // can only happen if the event loop is never blocked during an async call.
  let timerFiredDuringCall = false;
  const t0 = Date.now();
  const timer = new Promise<void>((r) =>
    setTimeout(() => {
      timerFiredDuringCall = true;
      r();
    }, 30)
  );
  const [p, q] = await Promise.all([native.multiplySlow(6, 7), native.multiplySlow(8, 9)]);
  const concurrentMs = Date.now() - t0;
  await timer;

  // Sync fn with a callback: the callback fires synchronously during the call,
  // so the steps are populated by the time it returns.
  const sumSteps: number[] = [];
  const sum = native.sumEach([10, 20, 30], (running) => sumSteps.push(running));

  const tsfnSteps: number[] = [];
  const tsfnSum = native.sumEachTsfn([10, 20, 30], (running) => tsfnSteps.push(running));

  const reversed = Array.from(native.reverseBytes(Buffer.from([1, 2, 3, 4])) as Uint8Array);
  const big = native.doubleBig(21n).toString();

  // #[napi(object)] value struct: returned by value as a typed object with
  // camelCase fields, and accepted back by value.
  const point = native.makePoint(2, 3, 'p');
  const pointLabel = point.labelText;
  const pointDesc = native.describePoint(point);

  // External<T> with a `&External<T>` param read provider-side via Deref.
  const image = native.imageMake(4, 5);
  const imageArea = native.imageArea(image);

  const handle = native.makeCounter(7);
  const counter = native.readCounter(handle);

  // Class: sync ctor + sync getter + async mutate/getter + async cross-method
  // class return, all through one proxy whose members are sync/async by their
  // Rust definition.
  const obj = new native.Counter(5);
  const afterAdd = await obj.addSlow(3);
  const value = obj.value;
  const child = await obj.forkSlow(100);
  const childValue = child.value;
  const parentUnchanged = obj.value;

  // Free-fn factory returning a class instance (the cross-class/factory path).
  const made = native.makeCounterClass(40);
  const madeValue = made.value;

  // A non-Clone/non-Serialize class returned by a free fn (by-move mint)...
  const tally = native.makeTally(11);
  const tallyTotal = tally.total;
  // ...and by a cross-class method (a method on Counter returning a Tally).
  const snap = obj.snapshot();
  const snapTotal = snap.total;

  // Class-level js_name divergence: TS sees RenamedBox, but provider dispatch
  // stays on BertBox.* wire names. Exercise ctor, method return, and factory
  // wrapping through the JS-facing class name.
  const renamed = new native.RenamedBox(12);
  const renamedAfterBump = renamed.bump(5);
  const renamedChild = renamed.duplicate();
  const renamedChildValue = renamedChild.value;
  const renamedMade = native.makeBertBox(30);
  const renamedMadeAfterBump = renamedMade.bump(2);

  // js_name divergence: a free fn (`bertShout`) and a method (`bertReset`) whose
  // JS names are deliberately not the camelCase of their Rust names (`shout`,
  // `reset`). They must be dispatched by the manifest's `rust_name`, not by
  // `camelToSnake(jsName)` — otherwise the call reaches an unknown function.
  const shout = native.bertShout('hi');
  const reset = obj.bertReset();

  return {
    role: process.env[SOCKET_ENV] ? 'rust-parent' : 'node-parent',
    add,
    greetNone,
    greetSome,
    scaleOmitted,
    scaleGiven,
    multiply: [p, q],
    concurrentMs,
    timerFiredDuringCall,
    sum,
    sumSteps,
    tsfnSum,
    tsfnSteps,
    reversed,
    big,
    pointLabel,
    pointDesc,
    imageArea,
    counter,
    afterAdd,
    value,
    childValue,
    parentUnchanged,
    madeValue,
    tallyTotal,
    snapTotal,
    renamedAfterBump,
    renamedChildValue,
    renamedMadeAfterBump,
    shout,
    reset,
  };
}

async function main(): Promise<void> {
  const provider = process.env[SOCKET_ENV]
    ? connectFromEnvSync()
    : launchProviderSync({ command: providerCommand() });
  let result;
  try {
    result = await exercise(provider);
  } finally {
    provider.close();
  }
  console.log('RESULT ' + JSON.stringify(result));
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
