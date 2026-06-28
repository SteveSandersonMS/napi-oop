// Synchronous variant: sync `add_numbers` blocks for a value; the async Rust fn
// stays a Promise even here. A worker thread owns the socket; the main thread
// blocks for sync calls. Node is always the parent.

import { join } from 'path';

import { launchProviderSync } from '@napi-oop/runtime';

import { bindSync } from './generated/bindings';

function providerCommand(): string {
  return join(__dirname, '..', '..', '..', 'target', 'release', 'add-numbers-provider');
}

async function main(): Promise<void> {
  const provider = launchProviderSync({ command: providerCommand() });
  try {
    const native = bindSync(provider);
    const result = native.addNumbers(2, 3);
    console.log(`[node-parent:sync] addNumbers(2, 3) = ${result}`);
    // Even under sync bindings, an async Rust fn stays a Promise — must await it.
    const product = await native.multiplySlow(6, 7);
    console.log(`[node-parent:sync] await multiplySlow(6, 7) = ${product}`);

    // Callbacks can't work while the main thread is blocked; sync mode rejects
    // them with a clear error rather than silently dropping them.
    try {
      native.sumEach([10, 20, 30], (running) => running);
    } catch (err) {
      console.log(`[node-parent:sync] sumEach with callback threw: ${(err as Error).message}`);
    }
  } finally {
    provider.close();
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
