import type { IncomingMessage, ServerResponse } from "node:http"
import { Server, createServer } from "node:https"

import { createLogger } from "@xen-ilp/lib-logger"
import { assertDefined, isObject } from "@xen-ilp/lib-type-utils"

import type { Config } from "../config"
import { MAX_BODY_SIZE } from "../constants/http"
import { XenMessage, parseMessage } from "../protocols/xen/message"
import type PeerManager from "./peer-manager"

const logger = createLogger("xen:node:http")

export interface HttpContext {
  config: Config
  peerManager: PeerManager
}

export class BadRequestError extends Error {
  readonly statusCode = 400
  readonly isOperational = true

  constructor(message: string) {
    super(message)
    this.name = "BadRequestError"
  }
}

export class NotAcceptableError extends Error {
  readonly statusCode = 406
  readonly isOperational = true

  constructor(message: string) {
    super(message)
    this.name = "NotAcceptableError"
  }
}

export class PayloadTooLargeError extends Error {
  readonly statusCode = 413
  readonly isOperational = true

  constructor(message: string) {
    super(message)
    this.name = "PayloadTooLargeError"
  }
}

export class UnsupportedMediaTypeError extends Error {
  readonly statusCode = 415
  readonly isOperational = true

  constructor(message: string) {
    super(message)
    this.name = "UnsupportedMediaTypeError"
  }
}

export default class HttpService {
  readonly server: Server

  constructor(readonly context: HttpContext) {
    this.context = context

    const port = context.config.port

    this.server = createServer(
      {
        cert: context.config.tlsWebCert,
        key: context.config.tlsWebKey,
      },
      this.handleRequest
    )

    this.server.listen(port)

    logger.info(
      `listening on https://${context.config.host}${
        port === 443 ? "" : `:${port}`
      }/`
    )
  }

  handleRequest = async (
    request: IncomingMessage,
    response: ServerResponse
  ) => {
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
          "/": this.handleGetRoot,
        },
        POST: {
          "/ilp": this.handlePostIlp,
          "/xen": this.handlePostXen,
        },
      }

      const send404 = () => {
        this.respondPlainly(response, 404, "Not Found")
      }

      // Call route handler or 404 if no matching route exists
      await (routes[request.method]?.[request.url] ?? send404)(
        request,
        response
      )
    } catch (error) {
      // Log any errors
      logger.logError(error, { skipAfter: "HttpService.handleRequest" })

      if (
        isObject(error) &&
        "statusCode" in error &&
        typeof error["statusCode"] === "number"
      ) {
        logger.debug(`responding with error`, {
          statusCode: error["statusCode"],
          message: error["message"],
        })
        this.respondPlainly(
          response,
          error["statusCode"],
          typeof error["message"] === "string" ? error["message"] : "Error"
        )
      } else {
        this.respondPlainly(response, 500, "Internal Server Error")
      }
    }
  }

  handleGetRoot = (_request: IncomingMessage, response: ServerResponse) => {
    response.writeHead(200, { "Content-Type": "text/html" })
    response.end(`Hello World!`)
  }

  handlePostIlp = (request: IncomingMessage, response: ServerResponse) => {
    this.assertAcceptHeader(request, "application/octet-stream")
    this.assertContentTypeHeader(request, "application/octet-stream")

    this.respondPlainly(
      response,
      500,
      "Internal Server Error, not yet implemented"
    )
  }

  handlePostXen = async (
    request: IncomingMessage,
    response: ServerResponse
  ) => {
    this.assertAcceptHeader(request, "application/xen-message")
    this.assertContentTypeHeader(request, "application/xen-message")

    const body = await this.parseBody(request)
    let message: XenMessage
    try {
      message = parseMessage(body)
    } catch (error) {
      throw new BadRequestError(`Bad Request, failed to parse message`)
    }
    this.context.peerManager.handleMessage(message)

    this.respondPlainly(response, 200, "OK")
  }

  async parseBody(request: IncomingMessage): Promise<Buffer> {
    const body: Array<Buffer> = []
    let dataReceived: number = 0

    for await (const chunk of request) {
      dataReceived += chunk.length

      if (dataReceived > MAX_BODY_SIZE) {
        logger.debug("request body too large")
        throw new PayloadTooLargeError(
          `Payload Too Large, expected body to be less than ${MAX_BODY_SIZE} bytes`
        )
      }
      body.push(chunk)
    }

    return Buffer.concat(body)
  }

  assertAcceptHeader(request: IncomingMessage, mediaType: string) {
    if (request.headers["accept"] !== mediaType) {
      throw new NotAcceptableError(`Not Acceptable, expected ${mediaType}`)
    }
  }

  assertContentTypeHeader(request: IncomingMessage, mediaType: string) {
    if (request.headers["content-type"] !== mediaType) {
      throw new UnsupportedMediaTypeError(
        `Unsupported Media Type, expected ${mediaType}`
      )
    }
  }

  respondPlainly(
    response: ServerResponse,
    statusCode: number,
    message: string
  ) {
    response.writeHead(statusCode, { "Content-Type": "text/plain" })
    response.end(message)
  }
}
