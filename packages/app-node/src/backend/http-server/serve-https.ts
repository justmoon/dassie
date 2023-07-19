import type { NextHandleFunction } from "connect"
import express, { Router } from "express"

import assert from "node:assert"
import type { IncomingMessage, ServerResponse } from "node:http"
import { createServer } from "node:https"
import type { Duplex } from "node:stream"

import { createLogger } from "@dassie/lib-logger"
import { createActor, createSignal } from "@dassie/lib-reactive"

import { databaseConfigSignal } from "../config/database-config"
import { getListenTargets } from "./utils/listen-targets"

const logger = createLogger("das:node:https-server")

export type Handler = (
  request: IncomingMessage,
  response: ServerResponse
) => void

export const additionalMiddlewaresSignal = () =>
  createSignal<NextHandleFunction[]>([])

export const httpsRouterService = () =>
  createActor<Router>(() => {
    return Router()
  })

export type WebsocketHandler = (
  request: IncomingMessage,
  socket: Duplex,
  head: Buffer
) => void

export const websocketRoutesSignal = () =>
  createSignal(new Map<string, WebsocketHandler>())

function handleError(error: unknown) {
  logger.error("https server error", { error })
}

export const httpsService = () =>
  createActor((sig) => {
    const config = sig.get(databaseConfigSignal)

    assert(config.hasTls, "Web UI is not configured")

    const router = sig.get(httpsRouterService)
    const additionalMiddlewares = sig.get(additionalMiddlewaresSignal)

    if (!router) return

    const { httpsPort, url, tlsWebCert, tlsWebKey } = config

    const app = express()

    app.use(router)

    for (const middleware of additionalMiddlewares) {
      app.use(middleware)
    }

    const server = createServer(
      {
        cert: tlsWebCert,
        key: tlsWebKey,
        requestCert: true,
        rejectUnauthorized: false,
      },
      app
    )

    for (const listenTarget of getListenTargets(httpsPort, true)) {
      server.listen(listenTarget)
    }

    logger.info(`listening on ${url}`)

    function handleUpgrade(
      request: IncomingMessage,
      socket: Duplex,
      head: Buffer
    ) {
      logger.debug("websocket upgrade request", { url: request.url })

      const { pathname } = new URL(request.url!, "http://localhost")

      const handler = sig.use(websocketRoutesSignal).read().get(pathname)

      if (!handler) {
        socket.destroy()
        return
      }

      handler(request, socket, head)
    }

    server.addListener("upgrade", handleUpgrade)
    server.addListener("error", handleError)

    sig.onCleanup(() => {
      server.removeListener("upgrade", handleUpgrade)
      server.removeListener("error", handleError)
      server.close()
    })

    return server
  })

export const serveHttps = () =>
  createActor((sig) => {
    sig.run(httpsRouterService)
    sig.run(httpsService)
  })
