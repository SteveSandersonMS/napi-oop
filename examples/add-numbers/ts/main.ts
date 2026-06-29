// Out-of-process entrypoint, symmetric in who is the parent:
//
// - Parent mode (run directly): launch the Rust provider as a child and call it.
// - Child mode (`NAPI_OOP_SOCKET` set, i.e. a Rust parent spawned us): connect
//   back to the parent and call it.
//
// One binding, faithful to native: sync Rust fns block for their value while
// `async` ones surface as non-blocking Promises. Either way the call logic — and
// the result — is identical.

import { join } from 'path';

import { SOCKET_ENV, connectFromEnvSync, launchProviderSync, type SyncProvider } from 'napi-oop-runtime';

import { bind } from './generated/bindings';

/** The provider binary built by `cargo build --release -p add-numbers-example`. */
function providerCommand(): string {
  return join(__dirname, '..', '..', '..', 'target', 'release', 'add-numbers-provider');
}

async function callAddNumbers(provider: SyncProvider): Promise<void> {
  const native = bind(provider);
  const role = process.env[SOCKET_ENV] ? 'rust-parent' : 'node-parent';

  // Sync Rust fn: blocks and returns the value directly.
  const result = native.addNumbers(2, 3);
  console.log(`[${role}] addNumbers(2, 3) = ${result}`);

  // Async Rust fn: surfaces as a Promise and dispatches without blocking the
  // event loop. Two 200ms calls overlap (~200ms total, not ~400ms).
  const t0 = Date.now();
  const [p, q] = await Promise.all([native.multiplySlow(6, 7), native.multiplySlow(8, 9)]);
  console.log(`[${role}] multiplySlow x2 => ${p}, ${q} in ${Date.now() - t0}ms (concurrent)`);

  // Callback param: Rust invokes the JS function once per step during the call.
  const steps: number[] = [];
  const total = native.sumEach([10, 20, 30], (running) => {
    steps.push(running);
  });
  console.log(`[${role}] sumEach => ${total}, steps=[${steps.join(', ')}]`);

  // The explicit ThreadsafeFunction form — same fire-and-forget semantics.
  const tsteps: number[] = [];
  const tt = native.sumEachTsfn([10, 20, 30], (running) => {
    tsteps.push(running);
  });
  console.log(`[${role}] sumEachTsfn => ${tt}, steps=[${tsteps.join(', ')}]`);
}

async function main(): Promise<void> {
  const provider = process.env[SOCKET_ENV]
    ? connectFromEnvSync()
    : launchProviderSync({ command: providerCommand() });
  try {
    await callAddNumbers(provider);
  } finally {
    provider.close();
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
