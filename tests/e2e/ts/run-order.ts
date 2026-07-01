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

  // The provider fires both callbacks from background threads, so "A" is only
  // guaranteed to reach the transport before "B" if there's enough real-time
  // separation to absorb OS thread-scheduling and timer jitter. What the test
  // must exercise is the *main-thread* hazard — "A" stranded on the async port
  // (event loop blocked) while "B" arrives over the sync port mid-call — which
  // holds for any positive separation. The transport itself can't reorder: the
  // worker stamps a monotonic `seq` in single-threaded socket-receive order and
  // the main thread delivers strictly in `seq` order. So an inversion can only
  // mean the provider genuinely wrote "B" before "A", i.e. the margins below were
  // too tight for the runner. Keep them generous (tuned for slow, jittery macOS
  // CI) so "A" is unambiguously fired and received first every iteration.
  const armDelayMs = 5; // "A" fires ~5ms after the provider receives orderArm
  const spinMs = 40; // block the event loop long enough that "A" is fired AND
  // received by the worker (stranded on the async port) before "B" is even armed
  const fireAtMs = 15; // "B" fires 15ms into the blocking call, i.e. well after "A"
  const blockMs = 40; // blocking-call duration; must outlast fireAtMs
  const settleCapMs = 1000; // generous upper bound for the async-port "A" to land

  for (let i = 0; i < iterations; i++) {
    const order: string[] = [];
    native.orderHold((_err: Error | null, label: string) => {
      order.push(label);
    });

    // (1) Arm "A" on the async port (no sync call in flight right now).
    native.orderArm(armDelayMs, 'A');

    // (2) Busy-wait so "A" lands on the async port but the loop can't drain it.
    const spinUntil = Date.now() + spinMs;
    while (Date.now() < spinUntil) {
      /* pure-JS spin: keep syncInFlight == 0 and the event loop blocked */
    }

    // (3) Blocking call fires "B" over the sync port, drained under Atomics.wait.
    native.orderBlockAndFire(blockMs, fireAtMs, 'B');

    // (4) Let the event loop deliver the stranded async-port "A". Wait until both
    //     callbacks have actually arrived rather than sleeping a fixed amount:
    //     what's under test is their *order*, not delivery latency, so a slow
    //     runner that merely delays "A" must not drop the iteration from the
    //     completed count. Each `await sleep` yields to the event loop so any
    //     queued async-port callback runs. Bounded so a genuine drop can't hang.
    const deadline = Date.now() + settleCapMs;
    while (order.length < 2 && Date.now() < deadline) {
      await sleep(2);
    }

    if (order.length === 2) {
      complete++;
      if (order[0] === 'B') inversions++;
    }
  }

  console.log('RESULT ' + JSON.stringify({ iterations, complete, inversions }));
  provider.close();
}

void main();
