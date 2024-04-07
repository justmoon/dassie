import express, { type RequestHandler } from "express"

import type { IncomingMessage, ServerResponse } from "node:http"
import { createServer } from "node:https"

import {
  createNodejsHttpHandlers,
  createRouter,
  createWebSocketRouter,
} from "@dassie/lib-http-server"
import { createActor, createSignal } from "@dassie/lib-reactive"

import { DatabaseConfigStore } from "../config/database-config"
import { http as logger } from "../logger/instances"
import { getListenTargets } from "./utils/listen-targets"

export type Handler = (
  request: IncomingMessage,
  response: ServerResponse,
) => void

export type ExpressMiddleware = RequestHandler

export const AdditionalMiddlewaresSignal = () =>
  createSignal<Array<ExpressMiddleware>>([])

export const HttpsRouter = () => createRouter()
export const HttpsWebSocketRouter = () => createWebSocketRouter()

function handleError(error: unknown) {
  logger.error("https server error", { error })
}

export const HttpsServiceActor = () =>
  createActor((sig) => {
    const { httpsPort, url, tlsWebCert, tlsWebKey } = sig.readKeysAndTrack(
      DatabaseConfigStore,
      ["httpsPort", "url", "tlsWebCert", "tlsWebKey"],
    )

    logger.assert(!!tlsWebCert, "Web UI is not configured, missing certificate")
    logger.assert(!!tlsWebKey, "Web UI is not configured, missing private key")

    const router = sig.reactor.use(HttpsRouter)
    const websocketRouter = sig.reactor.use(HttpsWebSocketRouter)
    const additionalMiddlewares = sig.readAndTrack(AdditionalMiddlewaresSignal)

    if (!router) return

    const app = express()

    for (const middleware of additionalMiddlewares) {
      app.use(middleware)
    }

    const server = createServer({
      cert: tlsWebCert,
      key: tlsWebKey,
    })

    for (const listenTarget of getListenTargets(httpsPort, true)) {
      server.listen(listenTarget)
    }

    logger.info(`listening on ${url}`)

    const nodejsHandlers = createNodejsHttpHandlers({
      onRequest: async (context) => router.handle(context),
      onUpgrade: async (context) => websocketRouter.handle(context),
      onError: handleError,
    })

    function handleRequest(
      nodeRequest: IncomingMessage,
      nodeResponse: ServerResponse<IncomingMessage>,
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

export const ServeHttpsActor = () =>
  createActor((sig) => {
    sig.run(HttpsServiceActor)
  })
