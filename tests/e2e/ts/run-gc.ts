// E2E driver for External GC release. Mints many External handles, drops the
// references, forces GC, and lets the runtime's FinalizationRegistry tell the
// provider to free each slab entry. Requires `node --expose-gc`. Prints the
// provider's live-handle count before and after collection.

import { join } from 'path';

import { launchProvider } from '@napi-oop/runtime';

import { bind, type Fixture, type ExternalObject } from './generated/bindings';

function providerCommand(): string {
  return join(__dirname, '..', '..', '..', 'target', 'release', 'e2e-provider');
}

const N = 500;

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function main(): Promise<void> {
  const provider = await launchProvider({ command: providerCommand() });
  try {
    const native: Fixture = bind(provider.peer);

    let handles: ExternalObject[] = [];
    for (let i = 0; i < N; i++) handles.push(await native.makeCounter(i));
    const before = await native.liveCounters();

    handles = [];
    for (let i = 0; i < 50 && (await native.liveCounters()) > 0; i++) {
      global.gc!();
      await sleep(20);
    }
    const after = await native.liveCounters();

    console.log('RESULT ' + JSON.stringify({ before, after }));
  } finally {
    await provider.close();
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
