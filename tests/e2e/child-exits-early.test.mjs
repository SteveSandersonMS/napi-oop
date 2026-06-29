// E2E: a Rust parent must not hang if its Node child exits before connecting.
// The child here exits immediately without ever dialing the socket (e.g. a
// startup crash, or a fast `--version`/`--help` path that never loads the
// runtime). `spawn_and_serve` polls accept against the child's exit, so the
// provider gives up and exits cleanly instead of parking in accept forever.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import { run } from './run.mjs';

const here = new URL('.', import.meta.url);
const ext = process.platform === 'win32' ? '.exe' : '';
const provider = fileURLToPath(new URL(`../../target/release/e2e-provider${ext}`, here));

test('rust-parent: child that exits before connecting does not hang the parent', async () => {
  // `run` rejects on timeout, so a hang fails the test rather than blocking it.
  const { code } = await run(provider, ['node', '-e', 'process.exit(0)'], here, 15000);
  assert.equal(code, 0, 'provider exits cleanly when the child never connects');
});
