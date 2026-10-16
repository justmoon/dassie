/**
 * The node's IPC socket as a transport for {@linkcode RpcClient}, outside
 * the browser: the CLI's way in, which the owner's tools and tests can use
 * too. Every message is a frame with a 4-byte big-endian length; whoever can
 * open the socket is the owner, so every call is authenticated.
 *
 * ```ts
 * const client = new RpcClient({ url: "ipc", connect: ipcConnector(path) });
 * ```
 *
 * @module
 */

import { connect, type Socket } from "node:net";
import type { SocketLike } from "./mod.ts";

/** The contract's limit on a message. */
const MAX_MESSAGE_BYTES = 1024 * 1024;
/** What may wait to be written before the node is taken to be stuck. */
const MAX_QUEUED_BYTES = 8 * 1024 * 1024;

const CONNECTING = 0;
const OPEN = 1;
const CLOSED = 3;

/** Makes {@linkcode RpcClient}'s sockets over the IPC socket at `path`. */
export function ipcConnector(path: string): () => SocketLike {
  return () => new IpcSocket(path);
}

class IpcSocket implements SocketLike {
  binaryType: "arraybuffer" | "blob" = "arraybuffer";
  readyState = CONNECTING;
  onopen: ((event: Event) => void) | null = null;
  onmessage: ((event: MessageEvent) => void) | null = null;
  onclose: ((event: CloseEvent) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  #socket: Socket;
  #buffered: Uint8Array<ArrayBuffer> = new Uint8Array(0);

  constructor(path: string) {
    const socket = connect({ path });
    this.#socket = socket;
    // A socket closed while connecting is destroyed, so it never connects.
    socket.once("connect", () => {
      this.readyState = OPEN;
      this.onopen?.(new Event("open"));
    });
    socket.on("data", (chunk: Uint8Array) => this.#received(chunk));
    socket.once("end", () => this.close(1006));
    // The connection failed; "close" follows.
    socket.on("error", () => {});
    socket.once("close", () => {
      if (this.readyState !== CLOSED) this.#closed(1006);
    });
  }

  send(data: Uint8Array): void {
    if (this.readyState !== OPEN) return;
    // What the kernel has not taken yet.
    if (this.#socket.writableLength + 4 + data.length > MAX_QUEUED_BYTES) {
      // The node does not read: give up rather than buffer without end.
      this.close(1006);
      return;
    }
    const frame = new Uint8Array(4 + data.length);
    new DataView(frame.buffer).setUint32(0, data.length);
    frame.set(data, 4);
    this.#socket.write(frame, (error) => {
      if (error) this.close();
    });
  }

  close(code = 1000): void {
    if (this.readyState === CLOSED) return;
    // Closed first: destroying fails the writes still queued, at once.
    this.readyState = CLOSED;
    this.#socket.destroy();
    this.#closed(code);
  }

  #received(chunk: Uint8Array): void {
    this.#buffered = concat(this.#buffered, chunk);
    try {
      while (this.readyState === OPEN && this.#buffered.length >= 4) {
        const buffered = this.#buffered;
        const length = new DataView(buffered.buffer).getUint32(0);
        if (length > MAX_MESSAGE_BYTES) throw new Error("frame too large");
        if (buffered.length < 4 + length) break;
        const message = buffered.slice(4, 4 + length);
        this.#buffered = buffered.slice(4 + length);
        this.onmessage?.(
          new MessageEvent("message", { data: message.buffer }),
        );
      }
    } catch {
      // A frame too large, or a message its handler could not take.
      this.close(1006);
    }
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
