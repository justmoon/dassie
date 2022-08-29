import type { WebSocket } from "ws"
import { WebSocketServer } from "ws"

import { createLogger } from "@dassie/lib-logger"
import { EffectContext, createService } from "@dassie/lib-reactive"

import { httpService } from "../http-server/serve-http"

const logger = createLogger("das:node:websocket-server")

export const registerUplinkHttpUpgrade = (sig: EffectContext) => {
  const websocketServer = sig.get(websocketService)
  const httpServer = sig.get(httpService)

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

export const websocketService = () =>
  createService((sig) => {
    const wss = new WebSocketServer({ noServer: true })

    wss.on("connection", handleConnection)

    sig.onCleanup(() => {
      wss.off("connection", handleConnection)
      wss.close()
    })

    return wss
  })
