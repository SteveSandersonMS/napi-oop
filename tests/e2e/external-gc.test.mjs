// E2E: External GC release. The provider's slab grows as handles are minted and
// must drain once the JS wrappers are collected and the FinalizationRegistry
// notifies the provider. Runs the driver with --expose-gc.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { run, parseResult } from './run.mjs';

const here = new URL('.', import.meta.url);

test('External handles release their slab entry after GC', async () => {
  const { code, out } = await run('node', ['--expose-gc', 'dist/run-gc.js'], here);
  assert.equal(code, 0, out);
  const r = parseResult(out);

  assert.equal(r.before, 500, `expected 500 live handles before GC, got ${r.before}`);
  assert.equal(r.after, 0, `expected slab to drain after GC, got ${r.after}`);
});
