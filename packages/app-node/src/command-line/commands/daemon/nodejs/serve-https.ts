import { createServer } from "node:https"

import { createNodejsHttpHandlers } from "@dassie/lib-http-server"
import { type Reactor, createActor } from "@dassie/lib-reactive"

import { DatabaseConfigStore } from "../../../../backend/config/database-config"
import { HttpsRouter } from "../../../../backend/http-server/values/https-router"
import { HttpsWebSocketRouter } from "../../../../backend/http-server/values/https-websocket-router"
import { http as logger } from "../../../../backend/logger/instances"

function handleError(error: unknown) {
  logger.error("https server error", { error })
}

export const SOCKET_ACTIVATION_NAME_HTTPS = "dassie-https.socket"

export const ServeHttpsActor = (reactor: Reactor) => {
  const httpsRouter = reactor.use(HttpsRouter)
  const websocketRouter = reactor.use(HttpsWebSocketRouter)

  return createActor((sig) => {
    const { httpsPort, url, tlsWebCert, tlsWebKey } = sig.readKeysAndTrack(
      DatabaseConfigStore,
      ["httpsPort", "url", "tlsWebCert", "tlsWebKey"],
    )

    // assert(logger, !!tlsWebCert, "Web UI is not configured, missing certificate")
    // assert(logger, !!tlsWebKey, "Web UI is not configured, missing private key")

    const server = createServer({
      cert: tlsWebCert,
      key: tlsWebKey,
    })

    server.listen(httpsPort)

    logger.info(`listening on ${url}`)

    const nodejsHandlers = createNodejsHttpHandlers({
      onRequest: async (context) => httpsRouter.handle(context),
      onUpgrade: async (context) => websocketRouter.handle(context),
      onError: handleError,
    })

    server.addListener("request", nodejsHandlers.handleRequest)
    server.addListener("upgrade", nodejsHandlers.handleUpgrade)
    server.addListener("error", nodejsHandlers.handleError)

    sig.onCleanup(() => {
      server.removeListener("request", nodejsHandlers.handleRequest)
      server.removeListener("upgrade", nodejsHandlers.handleUpgrade)
      server.removeListener("error", nodejsHandlers.handleError)
      server.close()
    })

    return server
  })
}
