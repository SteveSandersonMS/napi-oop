// E2E driver: exercises every cross-process flow napi-oop supports, then prints
// a single machine-readable `RESULT <json>` line the test harness asserts on.
// Symmetric in parentage: run directly (Node parent spawns Rust) or via the
// rust-parent launcher (Rust parent spawned us; NAPI_OOP_SOCKET is set).

import { join } from 'path';

import {
  Peer,
  SOCKET_ENV,
  connectFromEnv,
  launchProvider,
} from 'napi-oop-runtime';

import { bind } from './generated/bindings';

function providerCommand(): string {
  return join(__dirname, '..', '..', '..', 'target', 'release', 'e2e-provider');
}

async function exercise(peer: Peer) {
  const native = bind(peer);

  const add = await native.addNumbers(2, 3);

  // Concurrency: two 200ms async calls should overlap, finishing well under 400ms.
  const t0 = Date.now();
  const [p, q] = await Promise.all([native.multiplySlow(6, 7), native.multiplySlow(8, 9)]);
  const concurrentMs = Date.now() - t0;

  const sumSteps: number[] = [];
  const sum = await native.sumEach([10, 20, 30], (running) => sumSteps.push(running));

  const tsfnSteps: number[] = [];
  const tsfnSum = await native.sumEachTsfn([10, 20, 30], (running) => tsfnSteps.push(running));

  const reversed = Array.from(await native.reverseBytes(Buffer.from([1, 2, 3, 4])));

  const big = (await native.doubleBig(21n)).toString();

  const handle = await native.makeCounter(7);
  const counter = await native.readCounter(handle);

  // Async class: factory create, async getter, async cross-method class return.
  const obj = await native.Counter.create(5);
  const afterAdd = await obj.addSlow(3);
  const value = await obj.value;
  const child = await obj.forkSlow(100);
  const childValue = await child.value;
  const parentUnchanged = await obj.value;

  // Free-fn factory returning a class instance (the cross-class/factory path).
  const made = await native.makeCounterClass(40);
  const madeValue = await made.value;

  return {
    role: process.env[SOCKET_ENV] ? 'rust-parent' : 'node-parent',
    add,
    multiply: [p, q],
    concurrentMs,
    sum,
    sumSteps,
    tsfnSum,
    tsfnSteps,
    reversed,
    big,
    counter,
    afterAdd,
    value,
    childValue,
    parentUnchanged,
    madeValue,
  };
}

async function main(): Promise<void> {
  let result;
  if (process.env[SOCKET_ENV]) {
    const peer = await connectFromEnv();
    try {
      result = await exercise(peer);
    } finally {
      peer.close();
    }
  } else {
    const provider = await launchProvider({ command: providerCommand() });
    try {
      result = await exercise(provider.peer);
    } finally {
      await provider.close();
    }
  }
  console.log('RESULT ' + JSON.stringify(result));
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
