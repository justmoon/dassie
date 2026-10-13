/**
 * The node's IPC socket as a transport for {@linkcode RpcClient}, for Deno:
 * the CLI's way in, which the owner's tools and tests can use too. Every
 * message is a frame with a 4-byte big-endian length; whoever can open the
 * socket is the owner, so every call is authenticated.
 *
 * ```ts
 * const client = new RpcClient({ url: "ipc", connect: ipcConnector(path) });
 * ```
 *
 * @module
 */

import type { SocketLike } from "./mod.ts";

/** The contract's limit on a message. */
const MAX_MESSAGE_BYTES = 1024 * 1024;

const CONNECTING = 0;
const OPEN = 1;
const CLOSED = 3;

/** Makes {@linkcode RpcClient}'s sockets over the IPC socket at `path`. */
export function ipcConnector(path: string): () => SocketLike {
  return () => new IpcSocket(path);
}

class IpcSocket implements SocketLike {
  binaryType: BinaryType = "arraybuffer";
  readyState = CONNECTING;
  onopen: ((event: Event) => void) | null = null;
  onmessage: ((event: MessageEvent) => void) | null = null;
  onclose: ((event: CloseEvent) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  #connection: Deno.UnixConn | undefined;
  #writes: Promise<void> = Promise.resolve();

  constructor(path: string) {
    Deno.connect({ transport: "unix", path }).then(
      (connection) => {
        this.#connection = connection;
        this.readyState = OPEN;
        this.onopen?.(new Event("open"));
        this.#read(connection);
      },
      () => this.#closed(1006),
    );
  }

  send(data: Uint8Array): void {
    const connection = this.#connection;
    if (this.readyState !== OPEN || !connection) return;
    const frame = new Uint8Array(4 + data.length);
    new DataView(frame.buffer).setUint32(0, data.length);
    frame.set(data, 4);
    this.#writes = this.#writes
      .then(() => writeAll(connection, frame))
      .catch(() => this.close());
  }

  close(code = 1000): void {
    if (this.readyState === CLOSED) return;
    try {
      this.#connection?.close();
    } catch {
      // Closed already.
    }
    this.#closed(code);
  }

  async #read(connection: Deno.UnixConn): Promise<void> {
    let buffered: Uint8Array<ArrayBuffer> = new Uint8Array(0);
    const chunk = new Uint8Array(64 * 1024);
    try {
      for (;;) {
        const read = await connection.read(chunk);
        if (read === null) break;
        buffered = concat(buffered, chunk.subarray(0, read));
        for (;;) {
          if (buffered.length < 4) break;
          const length = new DataView(buffered.buffer, buffered.byteOffset)
            .getUint32(0);
          if (length > MAX_MESSAGE_BYTES) throw new Error("frame too large");
          if (buffered.length < 4 + length) break;
          const message = buffered.slice(4, 4 + length);
          buffered = buffered.slice(4 + length);
          this.onmessage?.(
            new MessageEvent("message", { data: message.buffer }),
          );
        }
      }
    } catch {
      // The connection failed; it closes below.
    }
    this.close(1006);
  }

  #closed(code: number): void {
    this.readyState = CLOSED;
    this.onclose?.(new CloseEvent("close", { code }));
  }
}

function concat(a: Uint8Array, b: Uint8Array): Uint8Array<ArrayBuffer> {
  const joined = new Uint8Array(a.length + b.length);
  joined.set(a);
  joined.set(b, a.length);
  return joined;
}

async function writeAll(connection: Deno.UnixConn, bytes: Uint8Array) {
  let written = 0;
  while (written < bytes.length) {
    written += await connection.write(bytes.subarray(written));
  }
}
