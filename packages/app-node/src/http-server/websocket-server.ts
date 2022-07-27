import type { WebSocket } from "ws"
import { WebSocketServer } from "ws"

import { createLogger } from "@xen-ilp/lib-logger"
import { createValue } from "@xen-ilp/lib-reactive"

import { httpServerValue } from "./http-server"

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

export const websocketServerValue = () =>
  createValue((sig) => {
    const httpServer = sig.get(httpServerValue)

    const wss = new WebSocketServer({ server: httpServer })

    wss.on("connection", handleConnection)

    sig.onCleanup(() => {
      wss.off("connection", handleConnection)
      wss.close()
    })

    return wss
  })
