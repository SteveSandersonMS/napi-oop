#!/usr/bin/env node
// CLI: generate TS bindings from a provider's type manifest.
//
//   napi-oop-codegen <provider-binary> <out-dir> [InterfaceName]
//
// Runs `<provider-binary> --emit-manifest`, then writes `<out-dir>/bindings.ts`
// (a self-contained module compiled by the consumer's tsc). This is the same
// source-of-truth flow napi-rs uses: the Rust signatures drive the generated
// TypeScript.

import { execFileSync } from 'child_process';
import { mkdirSync, writeFileSync } from 'fs';
import { join } from 'path';

import { generateTs, parseManifest } from './codegen';

function main(argv: string[]): void {
  const [binary, outDir, name = 'Bindings'] = argv;
  if (!binary || !outDir) {
    console.error('usage: napi-oop-codegen <provider-binary> <out-dir> [InterfaceName]');
    process.exit(2);
  }
  const json = execFileSync(binary, ['--emit-manifest'], { encoding: 'utf8' });
  const manifest = parseManifest(json);
  mkdirSync(outDir, { recursive: true });
  writeFileSync(join(outDir, 'bindings.ts'), generateTs(manifest, name));
  console.error(`generated ${manifest.functions.length} binding(s) -> ${outDir}/bindings.ts`);
}

main(process.argv.slice(2));
