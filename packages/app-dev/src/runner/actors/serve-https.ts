import express, { type RequestHandler } from "express"

import type { IncomingMessage, ServerResponse } from "node:http"
import { createServer } from "node:https"

import { DatabaseConfigStore } from "@dassie/app-node/src/backend/config/database-config"
import { HttpsRouter } from "@dassie/app-node/src/backend/http-server/values/https-router"
import { HttpsWebSocketRouter } from "@dassie/app-node/src/backend/http-server/values/https-websocket-router"
import { http as logger } from "@dassie/app-node/src/backend/logger/instances"
import { createNodejsHttpHandlers } from "@dassie/lib-http-server"
import { assert } from "@dassie/lib-logger"
import { createActor, createSignal } from "@dassie/lib-reactive"

export type ExpressMiddleware = RequestHandler

export const AdditionalMiddlewaresSignal = () =>
  createSignal<Array<ExpressMiddleware>>([])

function handleError(error: unknown) {
  logger.error("https server error", { error })
}

export const ServeHttpsActor = () =>
  createActor((sig) => {
    const { httpsPort, url, tlsWebCert, tlsWebKey } = sig.readKeysAndTrack(
      DatabaseConfigStore,
      ["httpsPort", "url", "tlsWebCert", "tlsWebKey"],
    )

    assert(
      logger,
      !!tlsWebCert,
      "Web UI is not configured, missing certificate",
    )
    assert(logger, !!tlsWebKey, "Web UI is not configured, missing private key")

    const router = sig.reactor.use(HttpsRouter)
    const websocketRouter = sig.reactor.use(HttpsWebSocketRouter)
    const additionalMiddlewares = sig.readAndTrack(AdditionalMiddlewaresSignal)

    const app = express()

    for (const middleware of additionalMiddlewares) {
      app.use(middleware)
    }

    const server = createServer({
      cert: tlsWebCert,
      key: tlsWebKey,
    })

    server.listen(httpsPort)

    logger.info(`listening on ${url}`)

    const nodejsHandlers = createNodejsHttpHandlers({
      onRequest: async (context) => router.handle(context),
      onUpgrade: async (context) => websocketRouter.handle(context),
      onError: handleError,
    })

    function handleRequest(
      nodeRequest: IncomingMessage,
      nodeResponse: ServerResponse,
    ) {
      if (router.match(nodeRequest.method!, nodeRequest.url!)) {
        nodejsHandlers.handleRequest(nodeRequest, nodeResponse)
      } else {
        app(nodeRequest, nodeResponse)
      }
    }

    server.addListener("request", handleRequest)
    server.addListener("upgrade", nodejsHandlers.handleUpgrade)
    server.addListener("error", nodejsHandlers.handleError)

    sig.onCleanup(() => {
      server.removeListener("request", handleRequest)
      server.removeListener("upgrade", nodejsHandlers.handleUpgrade)
      server.removeListener("error", nodejsHandlers.handleError)
      server.close()
    })

    return server
  })
