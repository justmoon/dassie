import { createServer } from "node:https"

import { createNodejsHttpHandlers } from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import type { DassieReactor } from "../../../../base/types/dassie-base"
import { DatabaseConfigStore } from "../../../../config/database-config"
import { FallbackToSelfSigned } from "../../../../http-server/functions/fallback-to-self-signed"
import { HttpsRouter } from "../../../../http-server/values/https-router"
import { HttpsWebSocketRouter } from "../../../../http-server/values/https-websocket-router"
import { http as logger } from "../../../../logger/instances"

function handleError(error: unknown) {
  logger.error("https server error", { error })
}

export const ServeHttpsActor = (reactor: DassieReactor) => {
  const httpsRouter = reactor.use(HttpsRouter)
  const websocketRouter = reactor.use(HttpsWebSocketRouter)
  const fallbackToSelfSigned = reactor.use(FallbackToSelfSigned)

  return createActor(async (sig) => {
    const { httpsPort, url, tlsWebCert, tlsWebKey } = sig.readKeysAndTrack(
      DatabaseConfigStore,
      ["httpsPort", "url", "tlsWebCert", "tlsWebKey"],
    )

    const { tlsWebCert: cert, tlsWebKey: key } = await fallbackToSelfSigned({
      tlsWebCert,
      tlsWebKey,
    })

    const server = createServer({
      cert,
      key,
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
