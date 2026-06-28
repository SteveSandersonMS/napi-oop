// Out-of-process entrypoint: launch the Rust provider as a child process and
// call its `#[napi]` functions over the named-socket transport.

import { join } from 'path';

import { createBinding, launchProvider } from '@napi-oop/runtime';

/** The functions the Rust provider exposes (Rust `add_numbers` -> `addNumbers`). */
interface AddNumbers {
  addNumbers(a: number, b: number): Promise<number>;
}

async function main(): Promise<void> {
  // The provider binary built by `cargo build --release -p add-numbers-example`.
  const command = join(
    __dirname,
    '..',
    '..',
    '..',
    'target',
    'release',
    'add-numbers-provider'
  );

  const provider = await launchProvider({ command });
  try {
    const native = createBinding<AddNumbers>(provider.peer);
    const a = 2;
    const b = 3;
    const result = await native.addNumbers(a, b);
    console.log(`addNumbers(${a}, ${b}) = ${result}`);
  } finally {
    await provider.close();
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
