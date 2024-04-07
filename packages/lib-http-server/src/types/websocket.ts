import type { Merge } from "type-fest"

import type { WebSocketResponse } from "../websocket/websocket-response"

export type WebSocketUpgradeHandler = (websocket: ServerWebSocket) => void

export interface WebSocketRequestContext {
  upgrade: (handler: WebSocketUpgradeHandler) => WebSocketResponse
}

export interface ServerWebSocket {
  addEventListener(
    eventType: "message",
    handler: (event: MessageEvent) => void,
  ): void
  addEventListener(eventType: "open" | "close", handler: () => void): void
  send(data: string | Uint8Array): void
  close(): void
  readyState: number
}

export interface MessageEvent {
  data: string | ArrayBuffer
}

export type WebSocketWithBinaryTypeArrayBuffer = Merge<
  WebSocket,
  {
    addEventListener(
      eventType: "message",
      handler: (event: MessageEvent) => void,
    ): void
    addEventListener(eventType: "open" | "close", handler: () => void): void
  }
>
