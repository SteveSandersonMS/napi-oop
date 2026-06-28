// @napi-oop/runtime — Node-side runtime for out-of-process napi.
//
// Connects to a Rust provider over a path-based named socket (UDS on Unix /
// named pipe on Windows via Node's `net`), never stdio. This phase implements
// the Node-as-parent bootstrap: generate a socket path, listen, spawn the Rust
// provider as a child that dials back, handshake, and expose async calls.
// Symmetric bootstrap (either side as parent) arrives in a later phase.

import { spawn, ChildProcess } from 'child_process';
import { randomBytes } from 'crypto';
import { createServer, Server, Socket } from 'net';
import { tmpdir } from 'os';
import { join } from 'path';
import { unlink } from 'fs/promises';

import { Peer } from './peer';

export { encodeFrame, createFrameDecoder } from './framing';
export { Peer } from './peer';
export { createBinding, camelToSnake } from './binding';
export {
  PROTOCOL_VERSION,
  type Hello,
  type Message,
  type Request,
  type Response,
  type ErrorMsg,
  type Role,
} from './messages';

/** Generate an unpredictable, platform-appropriate named-socket path. */
export function generateSocketPath(): string {
  const token = randomBytes(12).toString('hex');
  if (process.platform === 'win32') {
    return `\\\\.\\pipe\\napi-oop-${process.pid}-${token}`;
  }
  return join(tmpdir(), `napi-oop-${process.pid}-${token}.sock`);
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
  /** Extra arguments passed before the injected `connect <path>`. */
  args?: string[];
  /** Override the socket path (defaults to [`generateSocketPath`]). */
  socketPath?: string;
}

/**
 * Launch a Rust provider as a child process and connect to it.
 *
 * Node is the parent: it listens on a fresh named socket, spawns
 * `command [...args] connect <path>`, accepts the child's connection, and
 * completes the handshake. The child's stdio is inherited so its logs surface,
 * but the data channel is the socket only.
 */
export function launchProvider(options: LaunchOptions): Promise<Provider> {
  const socketPath = options.socketPath ?? generateSocketPath();
  const server: Server = createServer();

  return new Promise<Provider>((resolve, reject) => {
    server.on('error', reject);

    server.listen(socketPath, () => {
      const child = spawn(
        options.command,
        [...(options.args ?? []), 'connect', socketPath],
        { stdio: 'inherit' }
      );
      child.on('error', reject);

      server.once('connection', (socket: Socket) => {
        Peer.handshake(socket, 'caller').then((peer) => {
          const close = async (): Promise<void> => {
            peer.close();
            server.close();
            if (!child.killed) child.kill();
            if (process.platform !== 'win32') {
              await unlink(socketPath).catch(() => {});
            }
          };
          resolve({ peer, child, close });
        }, reject);
      });
    });
  });
}
