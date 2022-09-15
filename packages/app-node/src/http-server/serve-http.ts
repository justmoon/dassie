import connect, { NextHandleFunction } from "connect"
import createRouter from "find-my-way"

import type { IncomingMessage, ServerResponse } from "node:http"
import { createServer } from "node:https"

import { respondPlainly } from "@dassie/lib-http-server"
import { createLogger } from "@dassie/lib-logger"
import {
  EffectContext,
  createService,
  createSignal,
} from "@dassie/lib-reactive"
import { assertDefined, isObject } from "@dassie/lib-type-utils"

import { configSignal } from "../config"

const logger = createLogger("das:node:http-server")

export type Handler = (
  request: IncomingMessage,
  response: ServerResponse
) => void

const handleNotFound: Handler = (_request, response) => {
  respondPlainly(response, 404, "Not Found")
}

export const additionalMiddlewaresSignal = () =>
  createSignal<NextHandleFunction[]>([])

export const routerService = () =>
  createService(() => {
    return createRouter({
      defaultRoute: handleNotFound,
    })
  })

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

    const app = connect()

    for (const middleware of additionalMiddlewares) {
      app.use(middleware)
    }

    app.use(
      (request, response, next) =>
        void handleRequest(request, response).then(next)
    )

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
    async function handleRequest(
      request: IncomingMessage,
      response: ServerResponse
    ) {
      try {
        assertDefined(request.url)
        assertDefined(request.method)

        await router!.lookup(request, response)
      } catch (error) {
        // Log any errors
        logger.error(
          "error handling http request",
          { error },
          { skipAfter: "HttpService.handleRequest" }
        )

        if (
          isObject(error) &&
          "statusCode" in error &&
          typeof error["statusCode"] === "number"
        ) {
          logger.debug(`responding with error`, {
            statusCode: error["statusCode"],
            message: error["message"],
          })
          respondPlainly(
            response,
            error["statusCode"],
            typeof error["message"] === "string" ? error["message"] : "Error"
          )
        } else {
          respondPlainly(response, 500, "Internal Server Error")
        }
      }
    }

    sig.onCleanup(() => {
      server.close()
    })

    return server
  })

export const serveHttp = (sig: EffectContext) => {
  sig.run(sig.use(routerService).effect)
  sig.run(sig.use(httpService).effect)
}
