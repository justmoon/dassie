import express, { Router } from "express"

import type { IncomingMessage } from "node:http"
import { createServer } from "node:http"
import type { Duplex } from "node:stream"

import { createLogger } from "@dassie/lib-logger"
import { createActor, createSignal } from "@dassie/lib-reactive"

import { hasTlsComputed } from "../config/computed/has-tls"
import { databaseConfigStore } from "../config/database-config"
import { getListenTargets } from "./utils/listen-targets"

const logger = createLogger("das:node:http-server")

export const httpRouterService = () =>
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
  logger.error("http server error", { error })
}

export const httpService = () =>
  createActor((sig) => {
    const { httpPort, httpsPort, enableHttpServer } = sig.getKeys(
      databaseConfigStore,
      ["httpPort", "httpsPort", "enableHttpServer"]
    )
    const hasTls = sig.get(hasTlsComputed)
    const router = sig.get(httpRouterService)

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
          }${request.originalUrl}`
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

export const serveHttp = () =>
  createActor((sig) => {
    sig.run(httpRouterService)
    sig.run(httpService)
  })
