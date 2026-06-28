// Calls a tokio-based async Rust provider. The fn surfaces as Promise<number>;
// concurrent calls overlap on tokio's runtime. Symmetric bootstrap, like
// add-numbers: child when NAPI_OOP_SOCKET is set, else parent.

import { join } from 'path';

import { Peer, SOCKET_ENV, connectFromEnv, launchProvider } from '@napi-oop/runtime';

import { bind } from './generated/bindings';

function providerCommand(): string {
  return join(__dirname, '..', '..', '..', 'target', 'release', 'tokio-fetch-provider');
}

async function run(peer: Peer): Promise<void> {
  const native = bind(peer);
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
  if (process.env[SOCKET_ENV]) {
    const peer = await connectFromEnv();
    try {
      await run(peer);
    } finally {
      peer.close();
    }
    return;
  }
  const provider = await launchProvider({ command: providerCommand() });
  try {
    await run(provider.peer);
  } finally {
    await provider.close();
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
