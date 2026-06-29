// E2E driver for the synchronous bindings: the main thread blocks for sync
// results while a worker owns the socket. Async Rust fns stay Promises, and
// sync callbacks fire fire-and-forget between blocking calls. Node is parent.

import { join } from 'path';

import { launchProviderSync } from '@napi-oop/runtime';

import { bindSync } from './generated/bindings';

function providerCommand(): string {
  return join(__dirname, '..', '..', '..', 'target', 'release', 'e2e-provider');
}

async function main(): Promise<void> {
  const provider = launchProviderSync({ command: providerCommand() });
  try {
    const native = bindSync(provider);

    const add = native.addNumbers(2, 3);
    const product = await native.multiplySlow(6, 7);

    const steps: number[] = [];
    const sum = native.sumEach([10, 20, 30], (running) => steps.push(running)) as number;

    console.log('RESULT ' + JSON.stringify({ role: 'node-parent:sync', add, product, sum, steps }));
  } finally {
    provider.close();
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
