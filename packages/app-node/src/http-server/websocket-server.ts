import type { WebSocket } from "ws"
import { WebSocketServer } from "ws"

import { createLogger } from "@xen-ilp/lib-logger"
import type { EffectContext } from "@xen-ilp/lib-reactive"

import { httpServerStore } from "./http-server"

const logger = createLogger("xen:node:websocket-server")

const handleConnection = (socket: WebSocket) => {
  try {
    logger.debug("handle socket")

    socket.send("hello")
  } catch (error) {
    logger.error(
      "error handling websocket connection",
      { error },
      {
        skipAfter: "WebSocketService.handleConnection",
      }
    )
  }
}

export const websocketServer = (sig: EffectContext) => {
  const httpServer = sig.get(httpServerStore)

  if (!httpServer) return

  const wss = new WebSocketServer({ server: httpServer })

  wss.on("connection", handleConnection)

  sig.onCleanup(() => {
    wss.off("connection", handleConnection)
    wss.close()
  })
}
