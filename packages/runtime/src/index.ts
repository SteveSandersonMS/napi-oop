// napi-oop-runtime — Node-side runtime for out-of-process napi.
//
// Connects to a Rust provider over a path-based named socket (UDS on Unix /
// named pipe on Windows via Node's `net`), never stdio. The bootstrap is
// symmetric: either process may be the parent. The parent generates a socket
// path and exports it to the child via the `NAPI_OOP_SOCKET` env var; the child
// reads it and connects.

import { spawn, execFileSync, ChildProcess } from 'child_process';
import { randomBytes } from 'crypto';
import { connect as netConnect, createServer, Server, Socket } from 'net';
import { join } from 'path';
import { unlink } from 'fs/promises';

import { Peer } from './peer';
import { Role } from './messages';

export { encodeFrame, createFrameDecoder } from './framing';
export { Peer } from './peer';
export { camelToSnake } from './binding';
export { generateTs, parseManifest, type Manifest, type FnSignature } from './codegen';
export {
  launchProviderSync,
  connectFromEnvSync,
  createSyncBinding,
  bindClasses,
  type SyncProvider,
  type LaunchSyncOptions,
} from './sync';
export {
  PROTOCOL_VERSION,
  type Hello,
  type Message,
  type Request,
  type Response,
  type ErrorMsg,
  type Role,
} from './messages';

/** Opaque handle to a provider-side value (napi-rs `External<T>`). JS holds only
 *  a token; the value lives in the provider and is released when this is GC'd. */
export type ExternalObject = { readonly __napi_ext: number };

/** Env var a parent uses to pass the named-socket path to a spawned child. */
export const SOCKET_ENV = 'NAPI_OOP_SOCKET';

/** Whether this platform binds the transport in the Linux *abstract namespace*
 *  (an address with no filesystem entry) rather than a filesystem socket. */
function usesAbstractNamespace(): boolean {
  return process.platform === 'linux' || process.platform === 'android';
}

/** The real OS temp dir, deliberately ignoring the `TMPDIR`/`TMP`/`TEMP` env
 *  vars that `os.tmpdir()` honors: a consumer's harness may repoint them at a
 *  working directory, where the socket must never be created. Only reached on
 *  macOS/BSD — Linux uses the abstract namespace and Windows a named pipe. */
function realOsTempDir(): string {
  if (process.platform === 'darwin') {
    try {
      // The OS-provisioned per-user temp dir (`/var/folders/…/T/`), resolved
      // without consulting the environment.
      const dir = execFileSync('getconf', ['DARWIN_USER_TEMP_DIR'], {
        encoding: 'utf8',
      }).trim();
      if (dir) return dir;
    } catch {
      // Fall through to /tmp if getconf is unavailable.
    }
  }
  return '/tmp';
}

/** Map the logical socket name to the address Node's `net` expects: an
 *  abstract-namespace address (leading NUL) on Linux, otherwise the name as-is
 *  (a filesystem socket path on macOS/BSD, a named-pipe path on Windows). */
function socketAddress(name: string): string {
  return usesAbstractNamespace() ? `\0${name}` : name;
}

/**
 * Generate an unpredictable, platform-appropriate socket name.
 *
 * The name leaves no stray artifact in a directory a consumer might list:
 * a named pipe on Windows and an abstract-namespace socket on Linux (both have
 * no filesystem entry), and on macOS/BSD a socket file under the real OS temp
 * dir (never a `TMPDIR`-overridden one).
 */
export function generateSocketPath(): string {
  const token = randomBytes(12).toString('hex');
  if (process.platform === 'win32') {
    return `\\\\.\\pipe\\napi-oop-${process.pid}-${token}`;
  }
  if (usesAbstractNamespace()) {
    return `napi-oop-${process.pid}-${token}`;
  }
  return join(realOsTempDir(), `napi-oop-${process.pid}-${token}.sock`);
}

/**
 * Connect as the **child**: read the socket path the parent exported in
 * `SOCKET_ENV`, dial it, and complete the caller handshake. Used when a Rust
 * (or other) parent spawned this Node process.
 */
export function connectFromEnv(role: Role = 'caller'): Promise<Peer> {
  const socketPath = process.env[SOCKET_ENV];
  if (!socketPath) {
    return Promise.reject(
      new Error(`${SOCKET_ENV} not set; expected to be spawned as a child`)
    );
  }
  return connectPath(socketPath, role);
}

/** Connect to a peer listening at `socketPath` and complete the handshake. */
export function connectPath(socketPath: string, role: Role = 'caller'): Promise<Peer> {
  return new Promise((resolve, reject) => {
    const socket = netConnect(socketAddress(socketPath));
    socket.once('connect', () => {
      Peer.handshake(socket, role).then(resolve, reject);
    });
    socket.once('error', reject);
  });
}

/** A running provider child process plus the connected, handshaked [`Peer`]. */
export interface Provider {
  /** The connected peer; use `peer.call(fn, args)` to invoke functions. */
  readonly peer: Peer;
  /** The spawned child process. */
  readonly child: ChildProcess;
  /** Shut down the peer, server, and child, and clean up the socket file. */
  close(): Promise<void>;
}

/** Options for [`launchProvider`]. */
export interface LaunchOptions {
  /** The provider executable to spawn. */
  command: string;
  /** Arguments passed to the child (the socket path goes via the env var). */
  args?: string[];
  /** Override the socket path (defaults to [`generateSocketPath`]). */
  socketPath?: string;
}

/**
 * Launch a Rust provider as a child process and connect to it.
 *
 * Node is the parent: it listens on a fresh named socket, spawns `command`
 * (exporting the socket path in `SOCKET_ENV`), accepts the child's connection,
 * and completes the handshake. The child's stdio is inherited so its logs
 * surface, but the data channel is the socket only.
 */
export function launchProvider(options: LaunchOptions): Promise<Provider> {
  const socketPath = options.socketPath ?? generateSocketPath();
  const server: Server = createServer();

  return new Promise<Provider>((resolve, reject) => {
    server.on('error', reject);

    server.listen(socketAddress(socketPath), () => {
      // Spawn the provider in its own process group (`detached`). On a console
      // Ctrl+C the terminal delivers SIGINT/CTRL_C_EVENT only to the foreground
      // group; an isolated provider does not receive it and stays alive while the
      // Node parent runs its graceful shutdown — so native calls made during
      // shutdown (logging, secret filtering, server teardown) still succeed,
      // matching in-process behaviour where native code never disappears. The
      // provider exits on its own once the parent's socket closes (EOF). stdio is
      // still inherited, so it shares the parent console with no extra window.
      const child = spawn(options.command, options.args ?? [], {
        stdio: 'inherit',
        detached: true,
        env: { ...process.env, [SOCKET_ENV]: socketPath },
      });
      child.on('error', reject);

      server.once('connection', (socket: Socket) => {
        Peer.handshake(socket, 'caller').then((peer) => {
          const close = async (): Promise<void> => {
            peer.close();
            server.close();
            if (!child.killed) child.kill();
            // Only macOS/BSD create a filesystem socket to unlink; Linux uses
            // the abstract namespace and Windows a named pipe (no file).
            if (!usesAbstractNamespace() && process.platform !== 'win32') {
              await unlink(socketPath).catch(() => {});
            }
          };
          resolve({ peer, child, close });
        }, reject);
      });
    });
  });
}
