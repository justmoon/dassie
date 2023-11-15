import type { NextHandleFunction } from "connect"
import express from "express"

import assert from "node:assert"
import type { IncomingMessage, ServerResponse } from "node:http"
import { createServer } from "node:https"
import type { Duplex } from "node:stream"

import { createRouter } from "@dassie/lib-http-server"
import { createActor, createSignal } from "@dassie/lib-reactive"

import { DatabaseConfigStore } from "../config/database-config"
import { http as logger } from "../logger/instances"
import { getListenTargets } from "./utils/listen-targets"

export type Handler = (
  request: IncomingMessage,
  response: ServerResponse,
) => void

export const AdditionalMiddlewaresSignal = () =>
  createSignal<NextHandleFunction[]>([])

export const HttpsRouter = () => createRouter()

export type WebsocketHandler = (
  request: IncomingMessage,
  socket: Duplex,
  head: Buffer,
) => void

export const WebsocketRoutesSignal = () =>
  createSignal(new Map<string, WebsocketHandler>())

function handleError(error: unknown) {
  logger.error("https server error", { error })
}

export const HttpsServiceActor = () =>
  createActor((sig) => {
    const { httpsPort, url, tlsWebCert, tlsWebKey } = sig.getKeys(
      DatabaseConfigStore,
      ["httpsPort", "url", "tlsWebCert", "tlsWebKey"],
    )

    assert(tlsWebCert, "Web UI is not configured, missing certificate")
    assert(tlsWebKey, "Web UI is not configured, missing private key")

    const router = sig.use(HttpsRouter)
    const additionalMiddlewares = sig.get(AdditionalMiddlewaresSignal)

    if (!router) return

    const app = express()

    app.use(router.middleware)

    for (const middleware of additionalMiddlewares) {
      app.use(middleware)
    }

    const server = createServer(
      {
        cert: tlsWebCert,
        key: tlsWebKey,
      },
      app,
    )

    for (const listenTarget of getListenTargets(httpsPort, true)) {
      server.listen(listenTarget)
    }

    logger.info(`listening on ${url}`)

    function handleUpgrade(
      request: IncomingMessage,
      socket: Duplex,
      head: Buffer,
    ) {
      logger.debug("websocket upgrade request", { url: request.url })

      const { pathname } = new URL(request.url!, "http://localhost")

      const handler = sig.use(WebsocketRoutesSignal).read().get(pathname)

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

export const ServeHttpsActor = () =>
  createActor((sig) => {
    sig.run(HttpsServiceActor)
  })
