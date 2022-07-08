import type { IncomingMessage, ServerResponse } from "node:http"

import { createLogger } from "@xen-ilp/lib-logger"

import {
  MAX_BODY_SIZE,
  NotAcceptableError,
  PayloadTooLargeError,
  UnsupportedMediaTypeError,
} from "../constants/http"

const logger = createLogger("xen:node:http-utils")

export const assertAcceptHeader = (
  request: IncomingMessage,
  mediaType: string
) => {
  if (request.headers.accept !== mediaType) {
    throw new NotAcceptableError(`Not Acceptable, expected ${mediaType}`)
  }
}

export const assertContentTypeHeader = (
  request: IncomingMessage,
  mediaType: string
) => {
  if (request.headers["content-type"] !== mediaType) {
    throw new UnsupportedMediaTypeError(
      `Unsupported Media Type, expected ${mediaType}`
    )
  }
}

export const parseBody = async (request: IncomingMessage): Promise<Buffer> => {
  const body: Buffer[] = []
  let dataReceived = 0

  for await (const chunk of request as AsyncIterable<Buffer>) {
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

export const respondPlainly = (
  response: ServerResponse,
  statusCode: number,
  message: string
) => {
  response.writeHead(statusCode, { "Content-Type": "text/plain" })
  response.end(message)
}
