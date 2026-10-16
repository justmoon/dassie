/**
 * Where the dev environment keeps its files: under `local/dev/` in the
 * repository (ignored by git), or a directory of the caller's. The nodes'
 * IPC sockets live apart, in a short directory of their own, since a socket
 * path has a small limit.
 *
 * @module
 */

import process from "node:process";

/** The repository's root. */
export const ROOT: string = new URL("../../../", import.meta.url).pathname;

/**
 * The longest socket path that works everywhere: `sun_path` holds 104 bytes
 * on macOS (108 on Linux), with the closing NUL.
 */
export const MAX_SOCKET_PATH_BYTES = 103;

/**
 * What the node adds while it binds: it binds `<path>.<pid>.d/s` and then
 * renames it into place. A PID has at most 7 digits on Linux.
 */
export const SOCKET_SUFFIX_BYTES = ".1234567.d/s".length;

/** The dev environment's directories and files. */
export interface Layout {
  /** Everything the dev server keeps. */
  readonly state: string;
  /** The certificates, kept across runs. */
  readonly tls: string;
  /** The scenario chosen last. */
  readonly scenarioFile: string;
  /** The PIDs of the processes the dev server started. */
  readonly pidsFile: string;
  /** The PID of the dev server using it. */
  readonly lockFile: string;
  /** The nodes' data. */
  readonly data: string;
  /** The nodes' IPC sockets. */
  readonly sockets: string;
  /** mkcert's CA directory, if not its own default. */
  readonly caRoot?: string;
}

/** The layout under `state`, by default `local/dev/` in the repository. */
export async function layout(
  state = `${ROOT}local/dev`,
  caRoot?: string,
): Promise<Layout> {
  // A short name for the sockets' directory, the same for the same state
  // directory, so that runs side by side do not meet.
  const digest = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(state),
  );
  const tag = new Uint8Array(digest).toHex().slice(0, 12);
  const runtime = process.env.XDG_RUNTIME_DIR ?? "/tmp";
  return {
    state,
    tls: `${state}/tls`,
    scenarioFile: `${state}/scenario.txt`,
    pidsFile: `${state}/pids`,
    lockFile: `${state}/dev.pid`,
    data: `${state}/data`,
    sockets: `${runtime}/dassie-dev-${tag}`,
    caRoot,
  };
}

/** A node's data directory. */
export function dataDir(layout: Layout, name: string): string {
  return `${layout.data}/${name}`;
}

/** A node's IPC socket; throws if the path would be too long to bind. */
export function socketPath(layout: Layout, name: string): string {
  const path = `${layout.sockets}/${name}.sock`;
  checkSocketPath(path);
  return path;
}

/** Throws if the node could not bind a socket at `path`. */
export function checkSocketPath(path: string): void {
  const bytes = new TextEncoder().encode(path).length;
  if (bytes + SOCKET_SUFFIX_BYTES > MAX_SOCKET_PATH_BYTES) {
    throw new Error(
      `the socket path ${path} is too long (${bytes} bytes, at most ${
        MAX_SOCKET_PATH_BYTES - SOCKET_SUFFIX_BYTES
      }): set XDG_RUNTIME_DIR to a shorter directory`,
    );
  }
}
