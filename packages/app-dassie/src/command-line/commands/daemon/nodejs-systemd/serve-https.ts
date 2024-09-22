import { createServer } from "node:https"

import { createNodejsHttpHandlers } from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import type { SystemdReactor } from "."
import { DatabaseConfigStore } from "../../../../config/database-config"
import { FallbackToSelfSigned } from "../../../../http-server/functions/fallback-to-self-signed"
import { HttpsRouter } from "../../../../http-server/values/https-router"
import { HttpsWebSocketRouter } from "../../../../http-server/values/https-websocket-router"
import { http as logger } from "../../../../logger/instances"
import { getSocketActivationFileDescriptors } from "./socket-activation"

function handleError(error: unknown) {
  logger.error("https server error", { error })
}

const SOCKET_ACTIVATION_NAME_HTTPS = "dassie-https.socket"

export const ServeHttpsActor = (reactor: SystemdReactor) => {
  const httpsRouter = reactor.use(HttpsRouter)
  const websocketRouter = reactor.use(HttpsWebSocketRouter)
  const fallbackToSelfSigned = reactor.use(FallbackToSelfSigned)

  const server = createServer()

  const fileDescriptors = getSocketActivationFileDescriptors(
    reactor.base.socketActivationState,
    SOCKET_ACTIVATION_NAME_HTTPS,
  )

  logger.debug?.("using socket activation for https server", {
    fileDescriptors,
  })

  // socket activation can only be done once per process, which is why the call
  // to `server.listen` is outside of the createActor function
  for (const fd of fileDescriptors) {
    server.listen({ fd })
  }

  return createActor(async (sig) => {
    const { url, tlsWebCert, tlsWebKey } = sig.readKeysAndTrack(
      DatabaseConfigStore,
      ["url", "tlsWebCert", "tlsWebKey"],
    )

    const { tlsWebCert: cert, tlsWebKey: key } = await fallbackToSelfSigned({
      tlsWebCert,
      tlsWebKey,
    })

    server.setSecureContext({
      cert,
      key,
    })

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
