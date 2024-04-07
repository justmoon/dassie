import type { IncomingMessage, ServerResponse } from "node:http"
import type { Socket } from "node:net"

import { createContext } from "../../context"
import type { RequestContext } from "../../types/context"
import type { WebSocketRequestContext } from "../../types/websocket"
import { convertFromNodejsRequest, writeToNodejsResponse } from "./wrap-request"
import { createNodejsWebSocketServer } from "./wrap-websocket"

export interface NodejsWrapperOptions {
  hostname?: string | undefined
  onRequest?: ((context: RequestContext) => Promise<Response>) | undefined
  onUpgrade?:
    | ((context: RequestContext & WebSocketRequestContext) => Promise<Response>)
    | undefined
  onError?: ((error: unknown) => void) | undefined
}

export function createNodejsHttpHandlers({
  hostname,
  onRequest,
  onUpgrade,
  onError,
}: NodejsWrapperOptions) {
  const handleError =
    onError ??
    function handleError(error: unknown) {
      console.error("http server error", { error })
    }

  function handleRequest(
    nodeRequest: IncomingMessage,
    nodeResponse: ServerResponse<IncomingMessage>,
  ) {
    ;(async () => {
      const socket = nodeRequest.socket
      const request = convertFromNodejsRequest(nodeRequest, {
        hostname: hostname ?? "0.0.0.0",
        protocol: "encrypted" in socket && socket.encrypted ? "https" : "http",
      })
      let response: Response
      if (onRequest) {
        const context = createContext(request)
        response = await onRequest(context)
      } else {
        response = new Response("Not Found", { status: 404 })
      }

      await writeToNodejsResponse(response, nodeResponse)
    })().catch(handleError)
  }

  const createNodejsWebSocketContext = createNodejsWebSocketServer()

  function handleUpgrade(
    nodeRequest: IncomingMessage,
    socket: Socket,
    head: Buffer,
  ) {
    ;(async () => {
      const request = convertFromNodejsRequest(nodeRequest, {
        hostname: hostname ?? "0.0.0.0",
        protocol: "encrypted" in socket && socket.encrypted ? "https" : "http",
      })
      if (onUpgrade) {
        const context = Object.assign(
          createContext(request),
          createNodejsWebSocketContext(nodeRequest, socket, head),
        )

        await onUpgrade(context)
      } else {
        socket.destroy()
      }
    })().catch(handleError)
  }

  return {
    handleRequest,
    handleUpgrade,
    handleError,
  }
}
