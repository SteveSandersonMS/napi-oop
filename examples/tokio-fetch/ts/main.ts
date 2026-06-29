// Calls a tokio-based async Rust provider. `fetch_len` is an `async fn`, so the
// single binding surfaces it as `Promise<number>` and dispatches it without
// blocking the event loop; concurrent calls overlap on tokio's runtime.
// Symmetric bootstrap, like add-numbers: child when NAPI_OOP_SOCKET is set, else
// parent.

import { join } from 'path';

import { SOCKET_ENV, connectFromEnvSync, launchProviderSync, type SyncProvider } from 'napi-oop-runtime';

import { bind } from './generated/bindings';

function providerCommand(): string {
  return join(__dirname, '..', '..', '..', 'target', 'release', 'tokio-fetch-provider');
}

async function run(provider: SyncProvider): Promise<void> {
  const native = bind(provider);
  const role = process.env[SOCKET_ENV] ? 'rust-parent' : 'node-parent';
  const t0 = Date.now();
  const [a, b, c] = await Promise.all([
    native.fetchLen('https://example.com'),
    native.fetchLen('https://nodejs.org'),
    native.fetchLen('https://rust-lang.org'),
  ]);
  console.log(`[${role}:tokio] fetchLen x3 => ${a}, ${b}, ${c} in ${Date.now() - t0}ms`);
}

async function main(): Promise<void> {
  const provider = process.env[SOCKET_ENV]
    ? connectFromEnvSync()
    : launchProviderSync({ command: providerCommand() });
  try {
    await run(provider);
  } finally {
    provider.close();
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
