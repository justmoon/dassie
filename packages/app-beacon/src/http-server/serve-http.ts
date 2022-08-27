import createRouter from "find-my-way"

import type { IncomingMessage, ServerResponse } from "node:http"
import { createServer } from "node:https"

import { respondPlainly } from "@dassie/lib-http-server"
import { createLogger } from "@dassie/lib-logger"
import { EffectContext, createValue } from "@dassie/lib-reactive"
import { assertDefined, isObject } from "@dassie/lib-type-utils"

import { configStore } from "../config"

const logger = createLogger("das:beacon:http-server")

export type Handler = (
  request: IncomingMessage,
  response: ServerResponse
) => void

const handleGetRoot: Handler = (_request, response) => {
  response.writeHead(200, { "Content-Type": "text/html" })
  response.end(`Hello World!`)
}

const handleNotFound: Handler = (_request, response) => {
  respondPlainly(response, 404, "Not Found")
}

export const routerValue = () =>
  createValue(() => {
    return createRouter({
      defaultRoute: handleNotFound,
    })
  })

export const httpServerValue = () =>
  createValue((sig) => {
    const router = sig.get(routerValue)

    const { host, port, tlsWebCert, tlsWebKey } = sig.get(
      configStore,
      ({ host, port, tlsWebCert, tlsWebKey }) => ({
        host,
        port,
        tlsWebCert,
        tlsWebKey,
      })
    )

    const server = createServer(
      {
        cert: tlsWebCert,
        key: tlsWebKey,
        requestCert: true,
        rejectUnauthorized: false,
      },
      (...parameters) => void handleRequest(...parameters)
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

        await router.lookup(request, response)
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
  // Reading the HTTP server value will force it to be instantiated
  sig.read(httpServerValue)
}

export const registerRootRoute = (sig: EffectContext) => {
  const router = sig.get(routerValue)
  router.get("/", handleGetRoot)
}
