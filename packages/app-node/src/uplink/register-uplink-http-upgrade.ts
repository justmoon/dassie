import type { WebSocket } from "ws"
import { WebSocketServer } from "ws"

import { createLogger } from "@dassie/lib-logger"
import type { EffectContext } from "@dassie/lib-reactive"

import { httpService } from "../http-server/serve-http"

const logger = createLogger("das:node:websocket-server")

export const registerUplinkHttpUpgrade = (sig: EffectContext) => {
  const httpServer = sig.get(httpService)

  if (!httpServer) return

  const websocketServer = new WebSocketServer({ noServer: true })

  websocketServer.on("connection", handleConnection)

  sig.onCleanup(() => {
    websocketServer.off("connection", handleConnection)
    websocketServer.close()
  })

  httpServer.on("upgrade", (request, socket, head) => {
    if (request.url === "/uplink") {
      websocketServer.handleUpgrade(request, socket, head, (ws) => {
        websocketServer.emit("connection", ws, request)
      })
    }
  })
}

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
