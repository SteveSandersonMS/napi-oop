// Spawns a command, captures combined stdout/stderr, and resolves once it exits.
// Used by the E2E tests to run each example as a real, separate process and
// assert on its output — exercising the full socket/worker boundary, not mocks.

import { spawn } from 'node:child_process';

/** Run `command args` in `cwd`, resolving { code, out } once it exits (or rejecting on timeout). */
export function run(command, args, cwd, timeoutMs = 30000) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd });
    let out = '';
    child.stdout.on('data', (d) => (out += d));
    child.stderr.on('data', (d) => (out += d));
    const timer = setTimeout(() => {
      child.kill();
      reject(new Error(`timed out after ${timeoutMs}ms; output so far:\n${out}`));
    }, timeoutMs);
    child.on('error', reject);
    child.on('exit', (code) => {
      clearTimeout(timer);
      resolve({ code, out });
    });
  });
}

/** Extract and JSON-parse the single `RESULT <json>` line a driver prints. */
export function parseResult(out) {
  const line = out.split(/\r?\n/).find((l) => l.startsWith('RESULT '));
  if (!line) throw new Error(`no RESULT line in output:\n${out}`);
  return JSON.parse(line.slice('RESULT '.length));
}
