import express, { Router } from "express"

import type { IncomingMessage } from "node:http"
import { createServer } from "node:http"
import type { Duplex } from "node:stream"

import { createActor, createSignal } from "@dassie/lib-reactive"

import { HasTlsSignal } from "../config/computed/has-tls"
import { DatabaseConfigStore } from "../config/database-config"
import { http as logger } from "../logger/instances"
import { getListenTargets } from "./utils/listen-targets"

export const HttpRouterServiceActor = () =>
  createActor<Router>(() => {
    return Router()
  })

export type WebsocketHandler = (
  request: IncomingMessage,
  socket: Duplex,
  head: Buffer,
) => void

export const WebsocketRoutesSignal = () =>
  createSignal(new Map<string, WebsocketHandler>())

function handleError(error: unknown) {
  logger.error("http server error", { error })
}

export const HttpServiceActor = () =>
  createActor((sig) => {
    const { httpPort, httpsPort, enableHttpServer } = sig.getKeys(
      DatabaseConfigStore,
      ["httpPort", "httpsPort", "enableHttpServer"],
    )
    const hasTls = sig.get(HasTlsSignal)
    const router = sig.get(HttpRouterServiceActor)

    if (!router) return

    if (!enableHttpServer) return

    const app = express()

    app.use(router)

    // TODO: Maybe show some helpful message if TLS is not yet set up
    if (hasTls) {
      app.use("*", (request, response) => {
        response.redirect(
          `https://${request.hostname}${
            httpsPort === 443 ? "" : `:${httpsPort}`
          }${request.originalUrl}`,
        )
      })
    }

    const server = createServer({}, app)

    for (const listenTarget of getListenTargets(httpPort, false)) {
      server.listen(listenTarget)
    }

    server.addListener("error", handleError)

    sig.onCleanup(() => {
      server.removeListener("error", handleError)
      server.close()
    })

    return server
  })

export const ServeHttpActor = () =>
  createActor((sig) => {
    sig.run(HttpRouterServiceActor)
    sig.run(HttpServiceActor)
  })
