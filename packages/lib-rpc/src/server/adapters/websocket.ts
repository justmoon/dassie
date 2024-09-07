import { createTopic } from "@dassie/lib-reactive"

import type { WebSocketImplementation } from "../../common/types/websocket"
import type { InboundConnection } from "../server"

export function createWebSocketAdapter(
  websocket: WebSocketImplementation,
): InboundConnection {
  const received = createTopic<string>()

  websocket.addEventListener("message", ({ data }) => {
    if (typeof data !== "string") {
      throw new TypeError("Expected string data")
    }

    received.emit(data)
  })

  const closed = new Promise<void>((resolve) => {
    websocket.addEventListener("close", resolve)
  })

  return {
    received,
    send(data: string) {
      websocket.send(data)
    },
    closed,
  }
}
