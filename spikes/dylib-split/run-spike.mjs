// Orchestrates the dylib-split spike end to end, cross-platform.
//
// Two layouts depending on the target:
//   * shared-dylib (Windows/macOS/glibc-Linux): build with `-C prefer-dynamic`
//     so every artifact shares ONE dynamically-linked std; ship the shared core
//     dylib + that std + a thin .node + a thin provider binary.
//   * musl-static: musl cannot produce a Rust dylib, so statically link the core
//     into each wrapper and ship just a self-contained .node + provider binary
//     (size duplication on this platform only).
//
// Steps: build -> stage a clean dist/ -> (macOS) fix install names / rpaths and
// ad-hoc re-sign -> run the provider exe and `node require('./index.node')` from
// a FOREIGN cwd with library-path env vars CLEARED, proving the bits are
// self-contained. Exits non-zero on the first failure so CI fails loudly.

import { spawnSync } from "node:child_process";
import { mkdirSync, rmSync, copyFileSync, readdirSync, existsSync } from "node:fs";
import { join, dirname, basename } from "node:path";
import { fileURLToPath } from "node:url";
import { tmpdir, platform } from "node:os";

const HERE = dirname(fileURLToPath(import.meta.url));
const TARGET = join(HERE, "target", "release");
const DIST = join(HERE, "dist");
const OS = platform(); // 'win32' | 'darwin' | 'linux'

// musl ships no dynamic libstd, so the Rust `dylib` crate type is unsupported
// there. On musl we fall back to statically linking the business logic into each
// wrapper (one self-contained .node and one self-contained provider binary,
// accepting size duplication on that platform only). Everywhere else we use the
// shared-dylib layout (one shared core + one shared std).
const IS_MUSL =
  OS === "linux" &&
  (existsSync("/etc/alpine-release") ||
    !process.report?.getReport?.()?.header?.glibcVersionRuntime);
const SHARED = !IS_MUSL; // shared core dylib + dynamic std, vs musl static

function log(section) {
  console.log(`\n=== ${section} ===`);
}

function run(cmd, args, opts = {}) {
  console.log(`$ ${cmd} ${args.join(" ")}`);
  const r = spawnSync(cmd, args, { encoding: "utf8", ...opts });
  if (r.stdout) console.log(r.stdout.trimEnd());
  if (r.stderr) console.log(r.stderr.trimEnd());
  return r;
}

function must(cmd, args, opts = {}) {
  const r = run(cmd, args, opts);
  if (r.status !== 0) {
    console.error(`FAILED (exit ${r.status}): ${cmd} ${args.join(" ")}`);
    process.exit(1);
  }
  return r;
}

// --- platform-specific artifact + std library names -----------------------

function dylibName(libName) {
  if (OS === "win32") return `${libName}.dll`;
  if (OS === "darwin") return `lib${libName}.dylib`;
  return `lib${libName}.so`;
}

function findStdLib() {
  const sysroot = must("rustc", ["--print", "sysroot"]).stdout.trim();
  const targetLibdir = must("rustc", ["--print", "target-libdir"]).stdout.trim();
  const ext = OS === "win32" ? ".dll" : OS === "darwin" ? ".dylib" : ".so";
  const prefix = OS === "win32" ? "std-" : "libstd-";
  // The runtime shared std lives in target-libdir on unix, but in <sysroot>/bin
  // on Windows; search the likely locations and take the first match.
  const candidates = [targetLibdir, join(sysroot, "bin"), join(sysroot, "lib")];
  for (const dir of candidates) {
    let entries;
    try {
      entries = readdirSync(dir);
    } catch {
      continue;
    }
    const hit = entries.find((f) => f.startsWith(prefix) && f.endsWith(ext));
    if (hit) return join(dir, hit);
  }
  console.error(`Could not find dynamic std (${prefix}*${ext}) in any of:`);
  for (const dir of candidates) console.error(`  ${dir}`);
  process.exit(1);
}

// --- 1. build --------------------------------------------------------------

log(`BUILD (os=${OS}, mode=${SHARED ? "shared-dylib" : "musl-static"})`);

// In shared mode every artifact dynamically links one std (prefer-dynamic) and
// resolves siblings via rpath. In musl-static mode we link std + core
// statically; we must disable the default `crt-static` so the musl target can
// emit a cdylib (.node) and a normally-linked executable at all (they still link
// only the system musl libc, exactly like glibc binaries link system glibc).
let rustflags = "";
if (SHARED) {
  rustflags = "-C prefer-dynamic";
  if (OS === "linux") rustflags += " -C link-arg=-Wl,-rpath,$ORIGIN -C link-arg=-Wl,--enable-new-dtags";
  if (OS === "darwin") rustflags += " -C link-arg=-Wl,-rpath,@loader_path";
} else {
  rustflags = "-C target-feature=-crt-static";
}

// Build only the two wrappers. This pulls in their per-target `shared`
// dependency (the core dylib on dynamic targets, the core rlib on musl) and
// avoids building the dylib-only `core-dyn` crate on musl, where it cannot exist.
const buildEnv = { ...process.env, RUSTFLAGS: rustflags, RUSTC_WRAPPER: "" };
const build = run(
  "cargo",
  [
    "build",
    "--release",
    "-p",
    "spike-node-addon",
    "-p",
    "spike-provider",
    "--manifest-path",
    join(HERE, "Cargo.toml"),
  ],
  { env: buildEnv }
);
if (build.status !== 0) {
  console.error(`cargo build failed (exit ${build.status})`);
  process.exit(1);
}

// --- 2. stage --------------------------------------------------------------

log("STAGE");
rmSync(DIST, { recursive: true, force: true });
mkdirSync(DIST, { recursive: true });

const addonSrc = join(TARGET, dylibName("spike_node_addon"));
const provSrc = join(TARGET, OS === "win32" ? "spike-provider.exe" : "spike-provider");

const addonDst = join(DIST, "index.node");
const provDst = join(DIST, OS === "win32" ? "spike-provider.exe" : "spike-provider");

const staging = [
  [addonSrc, addonDst],
  [provSrc, provDst],
];

// In shared mode we also ship the one core dylib and the one dynamic std.
let coreDst = null;
let stdDst = null;
if (SHARED) {
  const coreSrc = join(TARGET, dylibName("spike_core_dyn"));
  const stdSrc = findStdLib();
  coreDst = join(DIST, basename(coreSrc));
  stdDst = join(DIST, basename(stdSrc));
  staging.push([coreSrc, coreDst], [stdSrc, stdDst]);
}

for (const [src, dst] of staging) {
  copyFileSync(src, dst);
  console.log(`staged ${basename(src)} -> ${dst}`);
}

// --- 3. macOS install-name / rpath fixups + ad-hoc signing ----------------

function machoDeps(file) {
  const r = run("otool", ["-L", file]);
  return (r.stdout || "")
    .split("\n")
    .slice(1) // first line is the filename
    .map((l) => l.trim().split(" ")[0])
    .filter(Boolean);
}

if (OS === "darwin" && SHARED) {
  log("MACOS FIXUPS");
  const coreBase = basename(coreDst);
  const stdBase = basename(stdDst);
  const rewriteTargets = new Set([coreBase, stdBase]);

  // Give the shared libs @rpath-relative ids.
  must("install_name_tool", ["-id", `@rpath/${coreBase}`, coreDst]);
  must("install_name_tool", ["-id", `@rpath/${stdBase}`, stdDst]);

  // Point every dependent's references to those libs at @rpath, and ensure each
  // has an @loader_path rpath so @rpath resolves to its own directory.
  for (const file of [coreDst, addonDst, provDst, stdDst]) {
    for (const dep of machoDeps(file)) {
      const b = basename(dep);
      if (rewriteTargets.has(b) && dep !== `@rpath/${b}`) {
        run("install_name_tool", ["-change", dep, `@rpath/${b}`, file]);
      }
    }
    // add_rpath fails if already present; ignore its error.
    run("install_name_tool", ["-add_rpath", "@loader_path", file]);
  }

  log("MACOS otool -L (post-fixup)");
  for (const file of [coreDst, addonDst, provDst]) machoDeps(file);

  log("MACOS ad-hoc codesign");
  for (const file of [coreDst, stdDst, addonDst, provDst]) {
    run("codesign", ["--force", "--sign", "-", file]);
  }
}

// --- diagnostics -----------------------------------------------------------

log("DIST CONTENTS");
for (const f of readdirSync(DIST)) console.log("  " + f);

log("LINKAGE DIAGNOSTICS");
const inspectTargets = SHARED ? [coreDst, addonDst, provDst] : [addonDst, provDst];
if (OS === "linux") {
  for (const f of inspectTargets) run("ldd", [f]);
} else if (OS === "darwin") {
  for (const f of inspectTargets) run("otool", ["-L", f]);
}

// --- 4. run with a foreign cwd and cleared library-path env ---------------

const cleanEnv = { ...process.env };
delete cleanEnv.LD_LIBRARY_PATH;
delete cleanEnv.DYLD_LIBRARY_PATH;
delete cleanEnv.DYLD_FALLBACK_LIBRARY_PATH;
const foreignCwd = tmpdir();

log("RUN provider (out-of-process host)");
const prov = run(provDst, [], { cwd: foreignCwd, env: cleanEnv });
if (prov.status !== 0) {
  console.error(`provider exited ${prov.status}`);
  process.exit(1);
}
const provOut = prov.stdout || "";
assertIncludes("provider", provOut, ["add=5", "reverse=[3, 2, 1]", "greeting=hello, provider"]);

log("RUN node addon (in-process host)");
const nodeScript = `
const addon = require(${JSON.stringify(addonDst)});
console.log("add=" + addon.add(40, 2));
const r = addon.reverseBytes([1, 2, 3]);
console.log("reverse=" + JSON.stringify(Array.from(r)));
console.log("greeting=" + addon.greeting("node"));
`;
const node = run(process.execPath, ["-e", nodeScript], { cwd: foreignCwd, env: cleanEnv });
if (node.status !== 0) {
  console.error(`node exited ${node.status}`);
  process.exit(1);
}
assertIncludes("node", node.stdout || "", ["add=42", "reverse=[3,2,1]", "greeting=hello, node"]);

log("SPIKE PASSED");

function assertIncludes(who, out, needles) {
  for (const n of needles) {
    if (!out.includes(n)) {
      console.error(`ASSERT FAILED (${who}): expected output to include ${JSON.stringify(n)}`);
      console.error(`actual:\n${out}`);
      process.exit(1);
    }
  }
  console.log(`${who} output OK (${needles.join(", ")})`);
}
