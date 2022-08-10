import { createLogger } from "@dassie/lib-logger"
import { EffectContext, createValue } from "@dassie/lib-reactive"
import type { WebSocket } from "ws"
import { WebSocketServer } from "ws"

import { httpServerValue } from "../http-server/serve-http"

const logger = createLogger("das:node:websocket-server")

export const registerUplinkHttpUpgrade = (sig: EffectContext) => {
  const websocketServer = sig.get(websocketServerValue)
  const httpServer = sig.get(httpServerValue)

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

export const websocketServerValue = () =>
  createValue((sig) => {
    const wss = new WebSocketServer({ noServer: true })

    wss.on("connection", handleConnection)

    sig.onCleanup(() => {
      wss.off("connection", handleConnection)
      wss.close()
    })

    return wss
  })

export const handleUplink = (sig: EffectContext) => {
  sig.use(websocketServerValue)
}
