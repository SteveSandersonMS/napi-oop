// Out-of-process entrypoint, symmetric in who is the parent:
//
// - Parent mode (run directly): launch the Rust provider as a child and call it.
// - Child mode (`NAPI_OOP_SOCKET` set, i.e. a Rust parent spawned us): connect
//   back to the parent and call it.
//
// Either way the call logic — and the result — is identical.

import { join } from 'path';

import {
  Peer,
  SOCKET_ENV,
  connectFromEnv,
  launchProvider,
} from '@napi-oop/runtime';

import { bind } from './generated/bindings';

/** The provider binary built by `cargo build --release -p add-numbers-example`. */
function providerCommand(): string {
  return join(__dirname, '..', '..', '..', 'target', 'release', 'add-numbers-provider');
}

async function callAddNumbers(peer: Peer): Promise<void> {
  const native = bind(peer);
  const a = 2;
  const b = 3;
  const result = await native.addNumbers(a, b);
  const role = process.env[SOCKET_ENV] ? 'rust-parent' : 'node-parent';
  console.log(`[${role}] addNumbers(${a}, ${b}) = ${result}`);
}

async function main(): Promise<void> {
  if (process.env[SOCKET_ENV]) {
    // Child: a Rust parent spawned us and is listening on the socket.
    const peer = await connectFromEnv();
    try {
      await callAddNumbers(peer);
    } finally {
      peer.close();
    }
    return;
  }

  // Parent: spawn the Rust provider as our child.
  const provider = await launchProvider({ command: providerCommand() });
  try {
    await callAddNumbers(provider.peer);
  } finally {
    await provider.close();
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
