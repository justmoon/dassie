import { WebSocketServer } from "ws"

import type { IncomingMessage } from "node:http"
import type { Socket } from "node:net"

import type {
  ServerWebSocket,
  WebSocketRequestContext,
  WebSocketUpgradeHandler,
} from "../../types/websocket"
import { WebSocketResponse } from "../../websocket/websocket-response"

export function createNodejsWebSocketServer() {
  const webSocketServer = new WebSocketServer({ noServer: true })

  return function createNodejsWebsocketContext(
    nodeRequest: IncomingMessage,
    socket: Socket,
    head: Buffer,
  ): WebSocketRequestContext {
    function upgrade(handler: WebSocketUpgradeHandler) {
      webSocketServer.handleUpgrade(
        nodeRequest,
        socket,
        head,
        (nodeWebSocket) => {
          nodeWebSocket.binaryType = "arraybuffer"

          handler(nodeWebSocket as ServerWebSocket)
        },
      )

      return new WebSocketResponse()
    }
    return {
      upgrade,
    }
  }
}
