import type { NextHandleFunction } from "connect"
import express, { Router } from "express"

import type { IncomingMessage, ServerResponse } from "node:http"
import { createServer } from "node:https"
import type { Duplex } from "node:stream"

import { createLogger } from "@dassie/lib-logger"
import {
  EffectContext,
  createService,
  createSignal,
} from "@dassie/lib-reactive"

import { configSignal } from "../config"

const logger = createLogger("das:node:http-server")

export type Handler = (
  request: IncomingMessage,
  response: ServerResponse
) => void

export const additionalMiddlewaresSignal = () =>
  createSignal<NextHandleFunction[]>([])

export const routerService = () =>
  createService<Router>(() => {
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
  createService((sig) => {
    const router = sig.get(routerService)
    const additionalMiddlewares = sig.get(additionalMiddlewaresSignal)

    if (!router) return

    const { host, port, tlsWebCert, tlsWebKey } = sig.getKeys(configSignal, [
      "host",
      "port",
      "tlsWebCert",
      "tlsWebKey",
    ])

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

export const serveHttp = (sig: EffectContext) => {
  sig.run(sig.use(routerService).effect)
  sig.run(sig.use(httpService).effect)
}
