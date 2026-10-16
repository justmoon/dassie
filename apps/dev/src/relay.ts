/**
 * The dashboard's way to one node (spec 7 §5): a WebSocket from the
 * dashboard's page, relayed message for message to the node's IPC socket,
 * where the node takes every call as its owner's. The dev server checks
 * who may open it (`server.ts`); nothing in the node accepts a dev token.
 *
 * @module
 */

import { SUBPROTOCOL } from "@dassie/rpc";
import { ipcConnector } from "@dassie/rpc/ipc";
import { OPEN, type Socket, upgradeWebSocket } from "./serve.ts";

/** The contract's limit on a message, both ways. */
export const MAX_MESSAGE_BYTES = 1024 * 1024;
/** What the page may have queued before it is dropped. */
const MAX_BUFFERED_BYTES = 8 * 1024 * 1024;
/** How long the node has to accept the connection. */
const CONNECT_TIMEOUT_MS = 5_000;

/**
 * Connects to the IPC socket at `path`, then upgrades `request` and relays
 * between the two until either closes. 502 if the node does not answer.
 * `onSocket` is told of the upgraded socket.
 */
export async function relay(
  request: Request,
  path: string,
  onSocket: (socket: Socket) => void = () => {},
): Promise<Response> {
  const ipc = ipcConnector(path)();
  const opened = await new Promise<boolean>((resolve) => {
    const timer = setTimeout(() => resolve(false), CONNECT_TIMEOUT_MS);
    ipc.onopen = () => {
      clearTimeout(timer);
      resolve(true);
    };
    ipc.onclose = () => {
      clearTimeout(timer);
      resolve(false);
    };
  });
  if (!opened) {
    ipc.close();
    return new Response("the node does not answer", { status: 502 });
  }
  let upgraded: ReturnType<typeof upgradeWebSocket>;
  try {
    // A larger message closes the socket with 1009 before it gets here.
    upgraded = upgradeWebSocket(request, {
      protocol: SUBPROTOCOL,
      maxPayload: MAX_MESSAGE_BYTES,
    });
  } catch {
    ipc.close();
    return new Response("not a WebSocket upgrade", { status: 400 });
  }
  const { socket, response } = upgraded;
  onSocket(socket);
  // The socket first: closing the IPC side calls `ipc.onclose` at once,
  // which would close the socket with its own code.
  const end = (code: number, reason: string) => {
    if (socket.readyState <= OPEN) socket.close(code, reason);
    ipc.close();
  };
  socket.onmessage = (event) => {
    if (!(event.data instanceof ArrayBuffer)) {
      return end(1003, "binary messages only");
    }
    ipc.send(new Uint8Array(event.data));
  };
  socket.onclose = () => ipc.close();
  socket.onerror = () => ipc.close();
  ipc.onmessage = (event) => {
    if (socket.readyState !== OPEN) return;
    if (socket.bufferedAmount > MAX_BUFFERED_BYTES) {
      return end(1013, "not reading");
    }
    socket.send(event.data as ArrayBuffer);
  };
  ipc.onclose = () => end(1001, "the node went away");
  return response;
}

/** Whether the client asked for the contract's subprotocol. */
export function wantsContract(request: Request): boolean {
  return (request.headers.get("sec-websocket-protocol") ?? "")
    .split(",")
    .some((protocol) => protocol.trim() === SUBPROTOCOL);
}
