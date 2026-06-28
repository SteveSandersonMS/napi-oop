// Synchronous variant: identical add_numbers call, but blocking — no Promises.
//
// A worker thread owns the socket and the Rust provider; the main thread blocks
// until each result is ready. Node is always the parent here.

import { join } from 'path';

import { launchProviderSync } from '@napi-oop/runtime';

import { bindSync } from './generated/bindings';

function providerCommand(): string {
  return join(__dirname, '..', '..', '..', 'target', 'release', 'add-numbers-provider');
}

const provider = launchProviderSync({ command: providerCommand() });
try {
  const native = bindSync(provider);
  const result = native.addNumbers(2, 3);
  console.log(`[node-parent:sync] addNumbers(2, 3) = ${result}`);
} finally {
  provider.close();
}
