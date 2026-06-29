// E2E driver for callback keep-alive. Registers a callback the provider stores
// past the call, then — in `release` mode — asks the provider to drop it. The
// process intentionally keeps *no other* live handle (the worker and async port
// are unref'd on their own), so whether it stays alive or exits is governed
// solely by the live callback's event-loop ref:
//   - `hold`:    the held callback must keep the loop alive (process does not exit).
//   - `release`: dropping it must let the loop drain (process exits cleanly).
// This mirrors how an in-process `ThreadsafeFunction` is ref'd by default until
// dropped — the behaviour a long-running out-of-process server depends on.

import { join } from 'path';

import { launchProviderSync } from 'napi-oop-runtime';

import { bind, type Fixture } from './generated/bindings';

function providerCommand(): string {
  return join(__dirname, '..', '..', '..', 'target', 'release', 'e2e-provider');
}

const mode = process.argv[2]; // 'hold' | 'release' | 'crash'

const provider = launchProviderSync({ command: providerCommand() });
const native: Fixture = bind(provider);

native.holdCallback(() => {});
if (mode === 'release') native.releaseCallback();
if (mode === 'crash') {
  try {
    // The provider exits mid-dispatch. This call must reject (not hang the main
    // thread on `Atomics.wait`), and the disconnect must release the keep-alive
    // ref the held callback took, so the process can exit despite holding it.
    native.exitProvider();
  } catch (err) {
    console.log('CALL_REJECTED ' + (err instanceof Error ? err.message : String(err)));
  }
}

// Deliberately do not close the provider and do not register any timer, socket,
// or listener. The only thing that can hold the loop open is the live callback.
console.log('READY ' + mode);
