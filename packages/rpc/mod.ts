/**
 * The browser's client for the node's RPC contract (`proto/dassie/ui/v1`,
 * spec 8 D5): one WebSocket to `/rpc`, binary protobuf frames.
 *
 * What the original's client got wrong, and this one does not (spec 2
 * §10.1, RT-27):
 *
 * - every call has a timeout, after which it fails and is cancelled on the
 *   node, instead of hanging forever when a response is lost;
 * - a lost connection fails the calls in flight, rather than re-sending
 *   them blindly after reconnecting, since a call may change the node;
 * - when the node ends the session (close code 4001) the client says so,
 *   so the page can return to the login screen.
 *
 * @example
 * ```ts
 * const client = new RpcClient({ url: "wss://node.example/rpc" });
 * const state = await client.call("getBasicState", {});
 * ```
 *
 * @module
 */

import {
  create,
  fromBinary,
  type MessageInitShape,
  toBinary,
} from "@bufbuild/protobuf";
import {
  ClientMessageSchema,
  ErrorCode,
  type RequestSchema,
  type Response,
  ServerMessageSchema,
  type StreamItem,
} from "./proto.ts";

export { ErrorCode } from "./proto.ts";

/** The contract's WebSocket subprotocol. */
export const SUBPROTOCOL = "dassie.ui.v1";

/** The close code with which the node ends a socket whose session ended. */
export const SESSION_ENDED = 4001;

type RequestInit = NonNullable<
  MessageInitShape<typeof RequestSchema>["method"]
>;
/** A method's name, as its request's oneof case. */
export type Method = Exclude<RequestInit["case"], undefined>;
/** What a method takes. */
export type RequestFor<M extends Method> = Extract<
  RequestInit,
  { case: M }
>["value"];
type ResponseMethod = Response["method"];
/** A method that answers once. */
export type UnaryMethod = Method & Exclude<ResponseMethod["case"], undefined>;
/** What a unary method answers. */
export type ResponseFor<M extends UnaryMethod> = Extract<
  ResponseMethod,
  { case: M }
>["value"];
type ItemMethod = StreamItem["method"];
/** A method that streams. */
export type StreamMethod = Method & Exclude<ItemMethod["case"], undefined>;
/** What a streaming method sends. */
export type ItemFor<M extends StreamMethod> = Extract<
  ItemMethod,
  { case: M }
>["value"];

/** A call failed. `code` says how; the message is for people. */
export class RpcError extends Error {
  /** The contract's error code. */
  readonly code: ErrorCode;

  constructor(code: ErrorCode, message: string) {
    super(message);
    this.name = "RpcError";
    this.code = code;
  }
}

/** The part of the WebSocket API the client uses. */
export interface SocketLike {
  binaryType: BinaryType;
  readonly readyState: number;
  onopen: ((event: Event) => void) | null;
  onmessage: ((event: MessageEvent) => void) | null;
  onclose: ((event: CloseEvent) => void) | null;
  onerror: ((event: Event) => void) | null;
  send(data: Uint8Array): void;
  close(code?: number, reason?: string): void;
}

/** How the client is set up. */
export interface RpcClientOptions {
  /** `wss://<host>/rpc`. */
  url: string;
  /** How long a unary call may take, in ms. Default 15 s. */
  callTimeoutMs?: number;
  /** How long a stream may go without a message, in ms. Default 5 min. */
  streamTimeoutMs?: number;
  /** Called when the node ends this browser's session. */
  onSessionEnded?: () => void;
  /** Makes the socket; tests pass a fake. */
  connect?: (url: string, protocol: string) => SocketLike;
}

interface Pending {
  onItem?: (item: ItemMethod) => void;
  settle: (outcome: { response?: ResponseMethod; error?: RpcError }) => void;
  timeoutMs: number;
  timer: ReturnType<typeof setTimeout> | undefined;
}

const OPEN = 1;
const BACKOFF_MIN_MS = 1_000;
const BACKOFF_MAX_MS = 30_000;

/** A connection to the node that reconnects as needed. */
export class RpcClient {
  readonly #options:
    & Required<Omit<RpcClientOptions, "onSessionEnded">>
    & RpcClientOptions;
  #socket: SocketLike | undefined;
  #queue: Uint8Array[] = [];
  #pending = new Map<number, Pending>();
  #nextCall = 1;
  #backoffMs = BACKOFF_MIN_MS;
  #reconnect: ReturnType<typeof setTimeout> | undefined;
  #closed = false;

  constructor(options: RpcClientOptions) {
    this.#options = {
      callTimeoutMs: 15_000,
      streamTimeoutMs: 300_000,
      connect: (url, protocol) => new WebSocket(url, protocol),
      ...options,
    };
  }

  /** Calls a unary method. */
  async call<M extends UnaryMethod>(
    method: M,
    request: RequestFor<M>,
  ): Promise<ResponseFor<M>> {
    const outcome = await this.#start(
      method,
      request,
      this.#options.callTimeoutMs,
    );
    if (outcome?.case !== method) {
      throw new RpcError(
        ErrorCode.INTERNAL,
        `the node answered ${outcome?.case} to ${method}`,
      );
    }
    return outcome.value as ResponseFor<M>;
  }

  /** Calls a streaming method, handing each item to `onItem`. */
  async stream<M extends StreamMethod>(
    method: M,
    request: RequestFor<M>,
    onItem: (item: ItemFor<M>) => void,
  ): Promise<void> {
    await this.#start(
      method,
      request,
      this.#options.streamTimeoutMs,
      (item) => {
        if (item.case === method) onItem(item.value as ItemFor<M>);
      },
    );
  }

  /** Closes the connection; calls in flight fail. */
  close(): void {
    this.#closed = true;
    clearTimeout(this.#reconnect);
    this.#socket?.close(1000);
    this.#failAll("the client was closed");
  }

  #start(
    method: Method,
    request: unknown,
    timeoutMs: number,
    onItem?: (item: ItemMethod) => void,
  ): Promise<ResponseMethod | undefined> {
    if (this.#closed) {
      return Promise.reject(
        new RpcError(ErrorCode.UNAVAILABLE, "the client is closed"),
      );
    }
    const callId = this.#nextCall;
    this.#nextCall = this.#nextCall >= 0xffff_ffff ? 1 : this.#nextCall + 1;
    const message = create(ClientMessageSchema, {
      callId,
      kind: {
        case: "request",
        value: { method: { case: method, value: request } as RequestInit },
      },
    });
    return new Promise((resolve, reject) => {
      const pending: Pending = {
        onItem,
        timeoutMs,
        timer: undefined,
        settle: ({ response, error }) => {
          clearTimeout(pending.timer);
          this.#pending.delete(callId);
          if (error) reject(error);
          else resolve(response);
        },
      };
      this.#pending.set(callId, pending);
      this.#arm(callId, pending);
      this.#send(toBinary(ClientMessageSchema, message));
    });
  }

  /** (Re)starts the call's timer; on expiry the call fails and is cancelled. */
  #arm(callId: number, pending: Pending): void {
    clearTimeout(pending.timer);
    pending.timer = setTimeout(() => {
      this.#send(
        toBinary(
          ClientMessageSchema,
          create(ClientMessageSchema, {
            callId,
            kind: { case: "cancel", value: {} },
          }),
        ),
      );
      pending.settle({
        error: new RpcError(
          ErrorCode.UNAVAILABLE,
          "the node did not answer in time",
        ),
      });
    }, pending.timeoutMs);
  }

  #send(bytes: Uint8Array): void {
    if (this.#socket?.readyState === OPEN) {
      this.#socket.send(bytes);
      return;
    }
    this.#queue.push(bytes);
    this.#open();
  }

  #open(): void {
    if (this.#socket || this.#reconnect !== undefined || this.#closed) return;
    const socket = this.#options.connect(this.#options.url, SUBPROTOCOL);
    socket.binaryType = "arraybuffer";
    this.#socket = socket;
    socket.onopen = () => {
      this.#backoffMs = BACKOFF_MIN_MS;
      for (const bytes of this.#queue.splice(0)) socket.send(bytes);
    };
    socket.onmessage = (event) => this.#receive(event.data);
    socket.onerror = () => {};
    socket.onclose = (event) => this.#lost(event.code);
  }

  #receive(data: unknown): void {
    if (!(data instanceof ArrayBuffer)) return;
    const message = fromBinary(ServerMessageSchema, new Uint8Array(data));
    const pending = this.#pending.get(message.callId);
    if (!pending) return;
    const kind = message.kind;
    switch (kind.case) {
      case "response":
        pending.settle({ response: kind.value.method });
        break;
      case "item":
        this.#arm(message.callId, pending);
        pending.onItem?.(kind.value.method);
        break;
      case "end":
        pending.settle({});
        break;
      case "error":
        pending.settle({
          error: new RpcError(kind.value.code, kind.value.message),
        });
        break;
    }
  }

  #lost(code: number): void {
    this.#socket = undefined;
    this.#failAll("the connection to the node was lost");
    if (code === SESSION_ENDED) this.#options.onSessionEnded?.();
    if (this.#closed) return;
    // The next connection waits out the backoff; calls made meanwhile queue
    // and go out once it opens, unless they time out first.
    const delay = this.#backoffMs * (0.5 + Math.random());
    this.#backoffMs = Math.min(this.#backoffMs * 2, BACKOFF_MAX_MS);
    this.#reconnect = setTimeout(() => {
      this.#reconnect = undefined;
      if (this.#queue.length > 0) this.#open();
    }, delay);
  }

  #failAll(message: string): void {
    for (const pending of [...this.#pending.values()]) {
      pending.settle({ error: new RpcError(ErrorCode.UNAVAILABLE, message) });
    }
    this.#queue = [];
  }
}
