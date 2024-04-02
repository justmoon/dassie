import express, { type RequestHandler } from "express"

import type { IncomingMessage, ServerResponse } from "node:http"
import { createServer } from "node:https"
import type { Duplex } from "node:stream"

import {
  convertFromNodejsRequest,
  createRouter,
  writeToNodejsResponse,
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
    const { httpsPort, url, tlsWebCert, tlsWebKey } = sig.readKeysAndTrack(
      DatabaseConfigStore,
      ["httpsPort", "url", "tlsWebCert", "tlsWebKey"],
    )

    logger.assert(!!tlsWebCert, "Web UI is not configured, missing certificate")
    logger.assert(!!tlsWebKey, "Web UI is not configured, missing private key")

    const router = sig.reactor.use(HttpsRouter)
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

    function handleRequest(
      nodeRequest: IncomingMessage,
      nodeResponse: ServerResponse<IncomingMessage>,
    ) {
      ;(async () => {
        const request = convertFromNodejsRequest(nodeRequest, {
          hostname: "0.0.0.0",
          protocol: "https",
        })
        const response = await router.handle(request)

        if (response.status === 404) {
          // Anything that isn't handled by our internal router is passed to Express
          app(nodeRequest, nodeResponse)
        } else {
          await writeToNodejsResponse(response, nodeResponse)
        }
      })().catch((error: unknown) => {
        logger.error("https server request error", { error })
      })
    }

    function handleUpgrade(
      request: IncomingMessage,
      socket: Duplex,
      head: Buffer,
    ) {
      logger.debug("websocket upgrade request", { url: request.url })

      const { pathname } = new URL(request.url!, "http://localhost")

      const handler = sig.read(WebsocketRoutesSignal).get(pathname)

      if (!handler) {
        socket.destroy()
        return
      }

      handler(request, socket, head)
    }

    server.addListener("request", handleRequest)
    server.addListener("upgrade", handleUpgrade)
    server.addListener("error", handleError)

    sig.onCleanup(() => {
      server.removeListener("request", handleRequest)
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
