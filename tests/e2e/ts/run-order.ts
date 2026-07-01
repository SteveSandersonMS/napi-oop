// E2E driver for cross-port callback ordering. Reproduces the hazard where a
// callback fired while a synchronous call is in flight (routed over the sync
// port, drained under `Atomics.wait`) can be delivered *before* an earlier
// callback still queued on the async port.
//
// Per iteration:
//   1. Arm callback "A" to fire ~5ms out, while no sync call is in flight, so it
//      is routed over the ASYNC port and queues on the (busy) event loop.
//   2. Busy-wait in pure JS so the event loop can't drain "A" yet.
//   3. Make a blocking sync call that fires "B" mid-call, over the SYNC port —
//      drained before the call returns, i.e. before the stranded "A".
//   4. Let the event loop deliver "A".
// A correct, globally-ordered transport always delivers [A, B]; the bug yields
// [B, A]. The driver prints how many inversions it observed.

import { join } from 'path';

import { launchProviderSync } from 'napi-oop-runtime';

import { bind, type Fixture } from './generated/bindings';

function providerCommand(): string {
  return join(__dirname, '..', '..', '..', 'target', 'release', 'e2e-provider');
}

const iterations = Number(process.argv[2] ?? '200');

const provider = launchProviderSync({ command: providerCommand() });
const native: Fixture = bind(provider);

const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));

async function main(): Promise<void> {
  let inversions = 0;
  let complete = 0;

  for (let i = 0; i < iterations; i++) {
    const order: string[] = [];
    native.orderHold((_err: Error | null, label: string) => {
      order.push(label);
    });

    // (1) Arm "A" on the async port (no sync call in flight right now).
    native.orderArm(5, 'A');

    // (2) Busy-wait so "A" lands on the async port but the loop can't drain it.
    const spinUntil = Date.now() + 12;
    while (Date.now() < spinUntil) {
      /* pure-JS spin: keep syncInFlight == 0 and the event loop blocked */
    }

    // (3) Blocking call fires "B" over the sync port, drained under Atomics.wait.
    native.orderBlockAndFire(20, 3, 'B');

    // (4) Let the event loop deliver the stranded async-port "A".
    await sleep(10);

    if (order.length === 2) {
      complete++;
      if (order[0] === 'B') inversions++;
    }
  }

  console.log('RESULT ' + JSON.stringify({ iterations, complete, inversions }));
  provider.close();
}

void main();
