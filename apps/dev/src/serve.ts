/**
 * `Deno.serve` and `Deno.upgradeWebSocket` for Node: a fetch-style handler
 * served over `node:http` or `node:https`, and WebSockets through `ws`, so
 * that a handler stays a function from `Request` to `Response`.
 *
 * - `serve` resolves once it listens, with the address it got (port 0
 *   picks one).
 * - A WebSocket upgrade reaches the handler as a `Request` too. If it
 *   answers with the response of {@linkcode upgradeWebSocket}, the
 *   handshake completes only then, so the socket stays CONNECTING until
 *   the handler returned, as in Deno; any other response is sent as it is
 *   and the connection closed.
 * - `shutdown` stops accepting, ends idle keep-alive connections, and
 *   destroys upgraded ones, which Node would otherwise wait for: their
 *   owners close them first.
 *
 * @module
 */

import { getRequestListener } from "@hono/node-server";
import {
  createServer as createHttpServer,
  type IncomingMessage,
} from "node:http";
import { createServer as createHttpsServer } from "node:https";
import type { AddressInfo } from "node:net";
import type { Duplex } from "node:stream";
import { type RawData, type WebSocket as Ws, WebSocketServer } from "ws";

export const CONNECTING = 0;
export const OPEN = 1;
export const CLOSING = 2;
export const CLOSED = 3;

/** The WebSocket a server holds: the part of the DOM's it uses. */
export interface Socket {
  /** Only `"arraybuffer"`: binary messages arrive as an `ArrayBuffer`. */
  binaryType: "arraybuffer";
  readonly readyState: number;
  /** Bytes queued and not yet handed to the operating system. */
  readonly bufferedAmount: number;
  onopen: ((event: Event) => void) | null;
  onmessage: ((event: MessageEvent) => void) | null;
  onclose: ((event: CloseEvent) => void) | null;
  onerror: ((event: Event) => void) | null;
  addEventListener(type: "close", listener: () => void): void;
  send(data: string | ArrayBuffer | Uint8Array): void;
  close(code?: number, reason?: string): void;
}

export interface ServeOptions {
  readonly hostname?: string;
  readonly port: number;
  /** PEM; with `key`, the server speaks TLS. */
  readonly cert?: string;
  readonly key?: string;
  /** The response to a request whose handler threw. */
  readonly onError?: (error: unknown) => Response;
}

export type Handler = (request: Request) => Response | Promise<Response>;

/** A server that listens. */
export interface Server {
  readonly addr: { readonly hostname: string; readonly port: number };
  /** Stops it; resolves once every connection is gone. */
  shutdown(): Promise<void>;
}

export interface UpgradeOptions {
  /** The subprotocol to agree to; the client must have offered it. */
  readonly protocol?: string;
  /** The largest message accepted; a larger one closes with 1009. */
  readonly maxPayload?: number;
}

/** An upgrade waiting for the handler's response. */
interface Pending {
  readonly message: IncomingMessage;
  readonly socket: Duplex;
  readonly head: Buffer;
  accepted?: {
    readonly socket: ServerSocket;
    readonly options: UpgradeOptions;
  };
}

const pending = new WeakMap<Request, Pending>();
const upgrades = new WeakSet<Response>();

/** Listens, and resolves once it does; rejects if it cannot. */
export function serve(
  options: ServeOptions,
  handler: Handler,
): Promise<Server> {
  const onError = options.onError ??
    (() => new Response("internal error", { status: 500 }));
  const guarded = async (request: Request) => {
    try {
      return await handler(request);
    } catch (error) {
      return onError(error);
    }
  };
  const listener = getRequestListener(guarded, {
    overrideGlobalObjects: false,
    errorHandler: (error) => onError(error),
  });
  const server = options.cert && options.key
    ? createHttpsServer({ cert: options.cert, key: options.key }, listener)
    : createHttpServer(listener);
  const upgraded = new Set<Duplex>();
  server.on("upgrade", (message: IncomingMessage, socket: Duplex, head) => {
    upgraded.add(socket);
    socket.once("close", () => upgraded.delete(socket));
    socket.on("error", () => socket.destroy());
    void upgrade(message, socket, head, guarded, options.cert !== undefined);
  });
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen({ host: options.hostname, port: options.port }, () => {
      server.off("error", reject);
      const address = server.address() as AddressInfo;
      resolve({
        addr: { hostname: address.address, port: address.port },
        shutdown() {
          const closed = new Promise<void>((done) =>
            server.close(() => done())
          );
          server.closeIdleConnections();
          for (const socket of upgraded) socket.destroy();
          return closed;
        },
      });
    });
  });
}

async function upgrade(
  message: IncomingMessage,
  socket: Duplex,
  head: Buffer,
  handler: Handler,
  secure: boolean,
): Promise<void> {
  const request = new Request(
    `${secure ? "https" : "http"}://${
      message.headers.host ?? "localhost"
    }${message.url}`,
    { method: message.method, headers: headersOf(message) },
  );
  const waiting: Pending = { message, socket, head };
  pending.set(request, waiting);
  const response = await handler(request);
  pending.delete(request);
  const accepted = waiting.accepted;
  if (upgrades.has(response) && accepted) {
    const server = new WebSocketServer({
      noServer: true,
      maxPayload: accepted.options.maxPayload,
      handleProtocols: () => accepted.options.protocol ?? false,
    });
    server.handleUpgrade(
      message,
      socket,
      head,
      (ws) => accepted.socket.attach(ws),
    );
    return;
  }
  await writeResponse(socket, response);
}

/**
 * Accepts the WebSocket upgrade of `request`, which {@linkcode serve}
 * handed to its handler. The handler must return `response`.
 */
export function upgradeWebSocket(
  request: Request,
  options: UpgradeOptions = {},
): { socket: Socket; response: Response } {
  const waiting = pending.get(request);
  if (!waiting) throw new TypeError("not a WebSocket upgrade");
  if (options.protocol !== undefined) {
    const offered = (request.headers.get("sec-websocket-protocol") ?? "")
      .split(",")
      .map((protocol) => protocol.trim());
    if (!offered.includes(options.protocol)) {
      throw new TypeError(`the client did not offer ${options.protocol}`);
    }
  }
  const socket = new ServerSocket();
  waiting.accepted = { socket, options };
  // A placeholder: the handshake itself is written by `ws`.
  const response = new Response(null, { status: 200 });
  upgrades.add(response);
  return { socket, response };
}

function headersOf(message: IncomingMessage): Headers {
  const headers = new Headers();
  for (let i = 0; i < message.rawHeaders.length; i += 2) {
    headers.append(message.rawHeaders[i], message.rawHeaders[i + 1]);
  }
  return headers;
}

/** Writes a plain HTTP response onto an upgrade's socket, and closes it. */
async function writeResponse(
  socket: Duplex,
  response: Response,
): Promise<void> {
  const body = new Uint8Array(await response.arrayBuffer());
  const lines = [`HTTP/1.1 ${response.status} ${response.statusText}`];
  response.headers.forEach((value, name) => lines.push(`${name}: ${value}`));
  lines.push(`content-length: ${body.length}`, "connection: close", "", "");
  socket.end(Buffer.concat([Buffer.from(lines.join("\r\n")), body]));
}

/** A `ws` socket behind the DOM's interface, once the handshake is done. */
class ServerSocket implements Socket {
  binaryType = "arraybuffer" as const;
  onopen: ((event: Event) => void) | null = null;
  onmessage: ((event: MessageEvent) => void) | null = null;
  onclose: ((event: CloseEvent) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  #ws?: Ws;
  #closing?: { code?: number; reason?: string };
  readonly #closeListeners: (() => void)[] = [];

  get readyState(): number {
    if (this.#ws) return this.#ws.readyState;
    return this.#closing ? CLOSED : CONNECTING;
  }

  get bufferedAmount(): number {
    return this.#ws?.bufferedAmount ?? 0;
  }

  addEventListener(_type: "close", listener: () => void): void {
    this.#closeListeners.push(listener);
  }

  send(data: string | ArrayBuffer | Uint8Array): void {
    if (this.#ws?.readyState !== OPEN) return;
    this.#ws.send(data);
  }

  close(code?: number, reason?: string): void {
    if (this.#ws) {
      if (this.#ws.readyState <= OPEN) this.#ws.close(code, reason);
      return;
    }
    // Before the handshake: it never opens.
    this.#closing = { code, reason };
  }

  attach(ws: Ws): void {
    if (this.#closing) {
      ws.close(this.#closing.code, this.#closing.reason);
      return;
    }
    this.#ws = ws;
    ws.on("message", (data: RawData, isBinary: boolean) => {
      this.onmessage?.(
        new MessageEvent("message", {
          data: isBinary ? arrayBuffer(data) : data.toString(),
        }),
      );
    });
    ws.on("error", () => this.onerror?.(new Event("error")));
    ws.on("close", (code: number, reason: Buffer) => {
      this.onclose?.(
        new CloseEvent("close", { code, reason: reason.toString() }),
      );
      for (const listener of this.#closeListeners) listener();
    });
    this.onopen?.(new Event("open"));
  }
}

/** The bytes of a binary message, as an `ArrayBuffer` of their own. */
function arrayBuffer(data: RawData): ArrayBuffer {
  const chunks = Array.isArray(data) ? data : [data];
  const bytes = Buffer.concat(
    chunks.map((chunk) =>
      chunk instanceof ArrayBuffer ? new Uint8Array(chunk) : chunk
    ),
  );
  return new Uint8Array(bytes).buffer;
}
