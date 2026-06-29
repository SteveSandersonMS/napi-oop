// E2E: callback keep-alive. A callback the provider still holds must keep the
// caller's event loop alive (like an in-process `ThreadsafeFunction`, ref'd by
// default), and releasing it must let the process exit. This is the property a
// long-running out-of-process server (whose accept callback lives provider-side)
// relies on to stay up.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';

const here = new URL('.', import.meta.url);

function spawnDriver(mode) {
  const child = spawn('node', ['dist/run-keepalive.js', mode], { cwd: here });
  let out = '';
  child.stdout.on('data', (d) => (out += d));
  child.stderr.on('data', (d) => (out += d));
  return { child, out: () => out };
}

test('a held callback keeps the process alive', async () => {
  const { child, out } = spawnDriver('hold');
  let exited = false;
  let exitCode;
  child.on('exit', (c) => {
    exited = true;
    exitCode = c;
  });

  // Long enough that, absent the keep-alive, the loop would have drained and the
  // process exited (the driver refs nothing else).
  await new Promise((r) => setTimeout(r, 1500));
  const alive = !exited;
  child.kill();

  assert.match(out(), /READY hold/, out());
  assert.ok(alive, `expected the process to stay alive while holding a callback, but it exited (code ${exitCode}); output:\n${out()}`);
});

test('releasing the held callback lets the process exit', async () => {
  const { child, out } = spawnDriver('release');
  const code = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      child.kill();
      reject(new Error(`process did not exit after releasing the callback; output:\n${out()}`));
    }, 8000);
    child.on('exit', (c) => {
      clearTimeout(timer);
      resolve(c);
    });
    child.on('error', reject);
  });

  assert.match(out(), /READY release/, out());
  assert.equal(code, 0, `expected a clean exit after releasing the callback; output:\n${out()}`);
});
