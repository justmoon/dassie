import { parse } from "cookie"
import { SuperJSON as superjson } from "superjson"
import type { WebSocket } from "ws"
import { WebSocketServer } from "ws"

import { Reactor, createActor } from "@dassie/lib-reactive"
import { createServer, createWebSocketAdapter } from "@dassie/lib-rpc/server"

import { SESSION_COOKIE_NAME } from "../../common/constants/cookie-name"
import { DEV_SECURITY_TOKEN_LENGTH } from "../../common/constants/general"
import { SessionsStore } from "../authentication/database-stores/sessions"
import { SessionToken } from "../authentication/types/session-token"
import { DassieActorContext } from "../base/types/dassie-base"
import { EnvironmentConfig } from "../config/environment-config"
import { WebsocketRoutesSignal } from "../http-server/serve-https"
import { appRouter } from "./app-router"

export const connectionMap = new Map<string, WebSocket>()

export const RegisterTrpcHttpUpgradeActor = (reactor: Reactor) => {
  const environmentConfig = reactor.use(EnvironmentConfig)
  const sessionsStore = reactor.use(SessionsStore)

  return createActor((sig: DassieActorContext) => {
    const websocketRoutes = sig.readAndTrack(WebsocketRoutesSignal)

    const rpcServer = createServer({
      router: appRouter,
      transformer: superjson,
    })

    const websocketServer = new WebSocketServer({
      noServer: true,
    })

    websocketRoutes.set("/trpc", (request, socket, head) => {
      websocketServer.handleUpgrade(request, socket, head, (websocket) => {
        const cookies = parse(request.headers.cookie ?? "")
        const sessionToken = cookies[SESSION_COOKIE_NAME]

        let authenticated = false

        if (
          sessionToken &&
          sessionsStore.read().has(sessionToken as SessionToken)
        ) {
          authenticated = true
        }

        // Alternative way to authenticate that can be used by the dev frontend
        // during development. This mode of authentication is not available in
        // production but if it somehow were to be enabled, it should still be
        // impossible to exploit because the expected token has to be provided
        // as an environment variable.
        if (import.meta.env.DEV) {
          const parsedUrl = new URL(request.url!, "http://localhost")
          const providedToken = parsedUrl.searchParams.get("token")
          const expectedToken = environmentConfig.devSecurityToken

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

        const context = {
          sig,
          isAuthenticated: authenticated,
        }

        rpcServer.handleConnection({
          connection: createWebSocketAdapter(websocket),
          context,
        })
      })
    })

    sig.onCleanup(() => {
      websocketRoutes.delete("/trpc")
      websocketServer.close()
    })
  })
}
