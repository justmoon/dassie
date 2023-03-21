import { applyWSSHandler } from "@trpc/server/adapters/ws"
import type { WebSocket } from "ws"
import { WebSocketServer } from "ws"

import { createActor } from "@dassie/lib-reactive"

import { websocketRoutesSignal } from "../http-server/serve-http"
import { appRouter } from "./app-router"
import { createContextFactory } from "./trpc-context"

export const connectionMap = new Map<string, WebSocket>()

export const registerTrpcHttpUpgrade = () =>
  createActor((sig) => {
    const websocketRoutes = sig.get(websocketRoutesSignal)

    const websocketServer = new WebSocketServer({
      noServer: true,
    })

    const handler = applyWSSHandler({
      wss: websocketServer,
      router: appRouter,
      createContext: createContextFactory(sig),
    })

    websocketRoutes.set("/trpc", (request, socket, head) => {
      websocketServer.handleUpgrade(request, socket, head, (ws) => {
        websocketServer.emit("connection", ws, request)
      })
    })

    sig.onCleanup(() => {
      websocketRoutes.delete("/trpc")
      handler.broadcastReconnectNotification()
      websocketServer.close()
    })
  })
