/**
 * The dashboard's side of the control channel (`proto/dassie/dev/v1`): one
 * WebSocket to the dev server at `/control`, opened again with a backoff
 * whenever it closes, since the dev server restarts often.
 *
 * Nothing here knows Solid; `store.ts` builds on it.
 */

import { create, type MessageInitShape } from "@bufbuild/protobuf";
import {
  ClientMessageSchema,
  decodeServerMessage,
  encodeClientMessage,
  type ServerMessage,
} from "@dassie/rpc/dev";

/** The part of a WebSocket the client uses. */
export interface SocketLike {
  binaryType: BinaryType;
  readyState: number;
  onopen: ((event: Event) => void) | null;
  onmessage: ((event: MessageEvent) => void) | null;
  onclose: ((event: CloseEvent) => void) | null;
  send(data: Uint8Array<ArrayBuffer>): void;
  close(): void;
}

export type ConnectionStatus = "connecting" | "open" | "closed";

export interface DevClientOptions {
  url: string;
  onMessage: (message: ServerMessage) => void;
  onStatus: (status: ConnectionStatus) => void;
  connect?: (url: string) => SocketLike;
  backoffMinMs?: number;
  backoffMaxMs?: number;
  random?: () => number;
}

export type Command = NonNullable<
  MessageInitShape<typeof ClientMessageSchema>["message"]
>;

const OPEN = 1;

export class DevClient {
  #socket?: SocketLike;
  #timer?: ReturnType<typeof setTimeout>;
  #attempt = 0;
  #closed = false;
  readonly options: DevClientOptions;

  constructor(options: DevClientOptions) {
    this.options = options;
  }

  start(): void {
    this.#open();
  }

  #open(): void {
    if (this.#closed) return;
    this.options.onStatus("connecting");
    const connect = this.options.connect ??
      ((url: string): SocketLike => new WebSocket(url));
    const socket = connect(this.options.url);
    socket.binaryType = "arraybuffer";
    this.#socket = socket;
    socket.onopen = () => {
      this.#attempt = 0;
      this.options.onStatus("open");
    };
    socket.onmessage = (event) => {
      if (!(event.data instanceof ArrayBuffer)) return;
      let message: ServerMessage;
      try {
        message = decodeServerMessage(new Uint8Array(event.data));
      } catch {
        socket.close();
        return;
      }
      this.options.onMessage(message);
    };
    socket.onclose = () => {
      if (this.#socket !== socket) return;
      this.#socket = undefined;
      this.options.onStatus("closed");
      this.#retry();
    };
  }

  #retry(): void {
    if (this.#closed) return;
    const min = this.options.backoffMinMs ?? 500;
    const max = this.options.backoffMaxMs ?? 10_000;
    const random = this.options.random ?? Math.random;
    const ceiling = Math.min(max, min * 2 ** this.#attempt);
    this.#attempt++;
    const delay = min + (ceiling - min) * random();
    this.#timer = setTimeout(() => this.#open(), delay);
  }

  /** Sends a command; false if the channel is not open. */
  send(command: Command): boolean {
    const socket = this.#socket;
    if (!socket || socket.readyState !== OPEN) return false;
    socket.send(
      encodeClientMessage(create(ClientMessageSchema, { message: command })),
    );
    return true;
  }

  close(): void {
    this.#closed = true;
    clearTimeout(this.#timer);
    this.#socket?.close();
    this.#socket = undefined;
  }
}
