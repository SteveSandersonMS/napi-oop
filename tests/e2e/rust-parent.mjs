// Cross-platform launcher for "Rust spawns Node" mode: resolve the provider
// binary (add .exe on Windows) and spawn it, telling it to launch the compiled
// run-all driver as its child. The driver detects NAPI_OOP_SOCKET and connects.
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const ext = process.platform === 'win32' ? '.exe' : '';
const provider = fileURLToPath(new URL(`../../target/release/e2e-provider${ext}`, import.meta.url));

const child = spawn(provider, ['node', 'dist/run-all.js'], { stdio: 'inherit' });
child.on('exit', (code) => process.exit(code ?? 1));
