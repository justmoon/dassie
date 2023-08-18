import { applyWSSHandler } from "@trpc/server/adapters/ws"
import { parse } from "cookie"
import type { WebSocket } from "ws"
import { WebSocketServer } from "ws"

import { createActor } from "@dassie/lib-reactive"

import { SESSION_COOKIE_NAME } from "../../common/constants/cookie-name"
import { sessionsStore } from "../authentication/database-stores/sessions"
import { SessionToken } from "../authentication/types/session-token"
import { websocketRoutesSignal } from "../http-server/serve-https"
import { appRouter } from "./app-router"

export const connectionMap = new Map<string, WebSocket>()

export const registerTrpcHttpUpgrade = () =>
  createActor((sig) => {
    const websocketRoutes = sig.get(websocketRoutesSignal)
    const sessions = sig.use(sessionsStore)

    const websocketServer = new WebSocketServer({
      noServer: true,
    })

    const handler = applyWSSHandler({
      wss: websocketServer,
      router: appRouter,
      createContext: ({ req }) => {
        const cookies = parse(req.headers.cookie ?? "")
        const sessionToken = cookies[SESSION_COOKIE_NAME]

        let authenticated = undefined

        if (sessionToken && sessions.read().has(sessionToken as SessionToken)) {
          authenticated = true as const
        }

        return {
          sig,
          user: authenticated,
        }
      },
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
