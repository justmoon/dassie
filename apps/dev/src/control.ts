/**
 * The dashboards' control channel (`proto/dassie/dev/v1`): every open
 * dashboard gets the state, the logs and the peerings on connecting, then
 * each change as it happens, and sends its commands back.
 *
 * A dashboard that stops reading is dropped rather than buffered for.
 *
 * @module
 */

import { create, type MessageInitShape, toBinary } from "@bufbuild/protobuf";
import {
  type ClientMessage,
  decodeClientMessage,
  encodeServerMessage,
  type Peerings,
  ServerMessageSchema,
  type SourcedLogLine,
  SourcedLogLineSchema,
  type State,
} from "@dassie/rpc/dev";

/** A server message's content, as created. */
export type Payload = MessageInitShape<typeof ServerMessageSchema>["message"];

/** What a dashboard may have queued before it is dropped. */
const MAX_BUFFERED_BYTES = 8 * 1024 * 1024;
/** The size of one message of logs, well below the frame limit. */
export const LOG_CHUNK_BYTES = 256 * 1024;
/** Dashboards open at once, at most. */
const MAX_CLIENTS = 32;

/** What a new dashboard is sent first. */
export interface ControlSource {
  state(): State;
  logs(): readonly SourcedLogLine[];
  peerings(): Peerings;
}

/** Splits `lines` into runs of at most `maxBytes` encoded. */
export function chunkLines(
  lines: readonly SourcedLogLine[],
  maxBytes = LOG_CHUNK_BYTES,
): SourcedLogLine[][] {
  const chunks: SourcedLogLine[][] = [];
  let current: SourcedLogLine[] = [];
  let size = 0;
  for (const line of lines) {
    // A field's tag and length, and the line.
    const bytes = toBinary(SourcedLogLineSchema, line).length + 6;
    if (current.length > 0 && size + bytes > maxBytes) {
      chunks.push(current);
      current = [];
      size = 0;
    }
    current.push(line);
    size += bytes;
  }
  if (current.length > 0) chunks.push(current);
  return chunks;
}

export class ControlHub {
  readonly #clients = new Set<WebSocket>();

  constructor(
    readonly source: ControlSource,
    readonly command: (message: ClientMessage) => void,
    readonly log: (message: string) => void,
  ) {}

  /** Serves a dashboard's socket, just upgraded. */
  attach(socket: WebSocket): void {
    socket.binaryType = "arraybuffer";
    socket.onopen = () => {
      if (this.#clients.size >= MAX_CLIENTS) {
        socket.close(1013, "too many dashboards");
        return;
      }
      this.#clients.add(socket);
      this.#greet(socket);
    };
    socket.onmessage = (event) => {
      if (!(event.data instanceof ArrayBuffer)) {
        socket.close(1003, "binary messages only");
        return;
      }
      let message: ClientMessage;
      try {
        message = decodeClientMessage(new Uint8Array(event.data));
      } catch (error) {
        socket.close(1007, "not a control message");
        this.log(`a dashboard sent a bad message: ${error}`);
        return;
      }
      this.command(message);
    };
    socket.onclose = () => this.#clients.delete(socket);
    socket.onerror = () => this.#clients.delete(socket);
  }

  #greet(socket: WebSocket): void {
    this.#send(socket, { case: "state", value: this.source.state() });
    const chunks = chunkLines(this.source.logs());
    if (chunks.length === 0) chunks.push([]);
    chunks.forEach((lines, index) =>
      this.#send(socket, {
        case: "logs",
        value: { reset: index === 0, lines },
      })
    );
    this.#send(socket, { case: "peerings", value: this.source.peerings() });
  }

  /** Sends `message` to every dashboard. */
  broadcast(message: Payload): void {
    if (this.#clients.size === 0) return;
    const bytes = encodeServerMessage(
      create(ServerMessageSchema, { message }),
    );
    for (const socket of this.#clients) this.#write(socket, bytes);
  }

  /** Sends new log lines to every dashboard, in chunks. */
  broadcastLogs(lines: readonly SourcedLogLine[]): void {
    for (const chunk of chunkLines(lines)) {
      this.broadcast({ case: "logs", value: { reset: false, lines: chunk } });
    }
  }

  #send(socket: WebSocket, message: Payload): void {
    this.#write(
      socket,
      encodeServerMessage(create(ServerMessageSchema, { message })),
    );
  }

  #write(socket: WebSocket, bytes: Uint8Array): void {
    if (socket.readyState !== WebSocket.OPEN) return;
    if (socket.bufferedAmount > MAX_BUFFERED_BYTES) {
      this.#clients.delete(socket);
      socket.close(1013, "not reading");
      return;
    }
    socket.send(bytes);
  }

  /** Closes every dashboard's socket. */
  close(): void {
    for (const socket of this.#clients) socket.close(1001, "shutting down");
    this.#clients.clear();
  }
}
