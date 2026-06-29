// Cross-platform launcher for the "Rust spawns Node" mode: resolves the
// platform-specific provider binary (adds .exe on Windows) and spawns it,
// telling it to launch `node dist/main.js` as the child.
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const ext = process.platform === 'win32' ? '.exe' : '';
const provider = fileURLToPath(new URL(`../../target/release/tokio-fetch-provider${ext}`, import.meta.url));

const child = spawn(provider, ['node', 'dist/main.js'], { stdio: 'inherit' });
child.on('exit', (code) => process.exit(code ?? 1));
