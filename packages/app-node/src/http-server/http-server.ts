import type { IncomingMessage, ServerResponse } from "node:http"
import { createServer } from "node:https"

import { createLogger } from "@xen-ilp/lib-logger"
import { createValue } from "@xen-ilp/lib-reactive"
import { assertDefined, isObject } from "@xen-ilp/lib-type-utils"

import { configStore } from "../config"
import {
  assertAcceptHeader,
  assertContentTypeHeader,
  parseBody,
  respondPlainly,
} from "../utils/http"
import { incomingXenMessageBufferTopic } from "../xen-protocol/topics/xen-protocol"

const logger = createLogger("xen:node:http-server")

const handleGetRoot = (_request: IncomingMessage, response: ServerResponse) => {
  response.writeHead(200, { "Content-Type": "text/html" })
  response.end(`Hello World!`)
}

const handlePostIlp = (request: IncomingMessage, response: ServerResponse) => {
  assertAcceptHeader(request, "application/octet-stream")
  assertContentTypeHeader(request, "application/octet-stream")

  respondPlainly(response, 500, "Internal Server Error, not yet implemented")
}

export const httpServerValue = () =>
  createValue((sig) => {
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

        const routes: Record<
          string,
          Record<
            string,
            (
              request: IncomingMessage,
              response: ServerResponse
            ) => Promise<void> | void
          >
        > = {
          GET: {
            "/": handleGetRoot,
          },
          POST: {
            "/ilp": handlePostIlp,
            "/xen": handlePostXen,
          },
        }

        const send404 = () => {
          respondPlainly(response, 404, "Not Found")
        }

        // Call route handler or 404 if no matching route exists
        await (routes[request.method]?.[request.url] ?? send404)(
          request,
          response
        )
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

    const handlePostXen = async (
      request: IncomingMessage,
      response: ServerResponse
    ) => {
      assertAcceptHeader(request, "application/xen-message")
      assertContentTypeHeader(request, "application/xen-message")

      const body = await parseBody(request)
      sig.emit(incomingXenMessageBufferTopic, body)

      respondPlainly(response, 200, "OK")
    }

    sig.onCleanup(() => {
      server.close()
    })

    return server
  })
