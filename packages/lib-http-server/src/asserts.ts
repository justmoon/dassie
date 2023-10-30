import type { IncomingMessage } from "node:http"

import { NotAcceptableError, UnsupportedMediaTypeError } from "./errors"

export const assertAcceptHeader = (
  request: IncomingMessage,
  mediaType: string,
) => {
  if (request.headers.accept !== mediaType) {
    throw new NotAcceptableError(`Not Acceptable, expected ${mediaType}`)
  }
}

export const assertContentTypeHeader = (
  request: IncomingMessage,
  mediaType: string,
) => {
  if (request.headers["content-type"] !== mediaType) {
    throw new UnsupportedMediaTypeError(
      `Unsupported Media Type, expected ${mediaType}`,
    )
  }
}
