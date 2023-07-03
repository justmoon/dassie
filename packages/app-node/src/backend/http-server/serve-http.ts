import type { NextHandleFunction } from "connect"
import express, { Router } from "express"

import assert from "node:assert"
import type { IncomingMessage, ServerResponse } from "node:http"
import { createServer } from "node:https"
import type { Duplex } from "node:stream"

import { createLogger } from "@dassie/lib-logger"
import { createActor, createSignal } from "@dassie/lib-reactive"

import { databaseConfigPlain } from "../config/database-config"
import { environmentConfigSignal } from "../config/environment-config"

const logger = createLogger("das:node:http-server")

export type Handler = (
  request: IncomingMessage,
  response: ServerResponse
) => void

export const additionalMiddlewaresSignal = () =>
  createSignal<NextHandleFunction[]>([])

export const routerService = () =>
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

export const httpService = () =>
  createActor((sig) => {
    const config = sig.use(databaseConfigPlain)

    assert(config.hasWebUi, "Web UI is not configured")

    const router = sig.get(routerService)
    const additionalMiddlewares = sig.get(additionalMiddlewaresSignal)

    if (!router) return

    const { host, port } = sig.getKeys(environmentConfigSignal, [
      "host",
      "port",
    ])

    const tlsWebCert = sig.get(config.tlsWebCert)
    const tlsWebKey = sig.get(config.tlsWebKey)

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

    server.listen(port)

    logger.info(
      `listening on https://${host}${port === 443 ? "" : `:${port}`}/`
    )

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

    function handleError(error: unknown) {
      logger.error("http server error", { error })
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

export const serveHttp = () =>
  createActor((sig) => {
    sig.run(routerService)
    sig.run(httpService)
  })
