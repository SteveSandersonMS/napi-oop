// Synchronous variant: identical add_numbers call, but blocking — no Promises.
//
// A worker thread owns the socket and the Rust provider; the main thread blocks
// until each result is ready. Node is always the parent here.

import { join } from 'path';

import { createSyncBinding, launchProviderSync } from '@napi-oop/runtime';

/** The provider functions, synchronous form. */
interface AddNumbers {
  addNumbers(a: number, b: number): number;
}

function providerCommand(): string {
  return join(__dirname, '..', '..', '..', 'target', 'release', 'add-numbers-provider');
}

const provider = launchProviderSync({ command: providerCommand() });
try {
  const native = createSyncBinding<AddNumbers>(provider);
  const result = native.addNumbers(2, 3);
  console.log(`[node-parent:sync] addNumbers(2, 3) = ${result}`);
} finally {
  provider.close();
}
