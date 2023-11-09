import type { IncomingMessage } from "node:http"

import { NotAcceptableFailure, UnsupportedMediaTypeFailure } from "."

export const assertAcceptHeader = (
  request: IncomingMessage,
  mediaType: string,
): NotAcceptableFailure | void => {
  if (request.headers.accept !== mediaType) {
    return new NotAcceptableFailure(`Not Acceptable, expected ${mediaType}`)
  }
}

export const assertContentTypeHeader = (
  request: IncomingMessage,
  mediaType: string,
): UnsupportedMediaTypeFailure | void => {
  if (request.headers["content-type"] !== mediaType) {
    return new UnsupportedMediaTypeFailure(
      `Unsupported Media Type, expected ${mediaType}`,
    )
  }
}
