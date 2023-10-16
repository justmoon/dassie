import { applyWSSHandler } from "@trpc/server/adapters/ws"
import { parse } from "cookie"
import type { WebSocket } from "ws"
import { WebSocketServer } from "ws"

import { createActor } from "@dassie/lib-reactive"

import { SESSION_COOKIE_NAME } from "../../common/constants/cookie-name"
import { DEV_SECURITY_TOKEN_LENGTH } from "../../common/constants/general"
import { SessionsStore } from "../authentication/database-stores/sessions"
import { SessionToken } from "../authentication/types/session-token"
import { EnvironmentConfigSignal } from "../config/environment-config"
import { WebsocketRoutesSignal } from "../http-server/serve-https"
import { appRouter } from "./app-router"

export const connectionMap = new Map<string, WebSocket>()

export const RegisterTrpcHttpUpgradeActor = () =>
  createActor((sig) => {
    const websocketRoutes = sig.get(WebsocketRoutesSignal)
    const sessions = sig.use(SessionsStore)

    const websocketServer = new WebSocketServer({
      noServer: true,
    })

    const handler = applyWSSHandler({
      wss: websocketServer,
      router: appRouter,
      createContext: ({ req: { headers, url } }) => {
        const cookies = parse(headers.cookie ?? "")
        const sessionToken = cookies[SESSION_COOKIE_NAME]

        let authenticated = false

        if (sessionToken && sessions.read().has(sessionToken as SessionToken)) {
          authenticated = true
        }

        // Alternative way to authenticate that can be used by the dev frontend
        // during development. This mode of authentication is not available in
        // production but if it somehow were to be enabled, it should still be
        // impossible to exploit because the expected token has to be provided
        // as an environment variable.
        if (import.meta.env.DEV) {
          const parsedUrl = new URL(url!, "http://localhost")
          const providedToken = parsedUrl.searchParams.get("token")
          const expectedToken = sig
            .use(EnvironmentConfigSignal)
            .read().devSecurityToken

          if (
            expectedToken &&
            expectedToken.length === DEV_SECURITY_TOKEN_LENGTH &&
            providedToken &&
            providedToken.length === DEV_SECURITY_TOKEN_LENGTH &&
            providedToken === expectedToken
          ) {
            authenticated = true
          }
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
