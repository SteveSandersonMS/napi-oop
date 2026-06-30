// Stages the dual-ABI cdylib as `fixture.node` so Node can `require()` it as an
// in-process napi addon (the in-proc door of the same artifact the provider exe
// dlopens for the out-of-process door). Resolves the platform-specific library
// name produced by `cargo build --release -p napi-oop-e2e-fixture`.

import { copyFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const releaseDir = join(here, '..', '..', 'target', 'release');

const libName =
  process.platform === 'win32'
    ? 'napi_oop_e2e_fixture.dll'
    : process.platform === 'darwin'
      ? 'libnapi_oop_e2e_fixture.dylib'
      : 'libnapi_oop_e2e_fixture.so';

const src = join(releaseDir, libName);
const dest = join(here, 'fixture.node');
copyFileSync(src, dest);
console.log(`staged ${src} -> ${dest}`);
