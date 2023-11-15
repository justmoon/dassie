import type { IncomingMessage } from "node:http"

import { NotAcceptableFailure, UnsupportedMediaTypeFailure } from "."

export const createAcceptHeaderAssertion =
  (mediaType: string) =>
  (request: IncomingMessage): NotAcceptableFailure | void => {
    if (request.headers.accept !== mediaType) {
      return new NotAcceptableFailure(`Not Acceptable, expected ${mediaType}`)
    }
  }

export const createContentTypeHeaderAssertion =
  (mediaType: string) =>
  (request: IncomingMessage): UnsupportedMediaTypeFailure | void => {
    if (request.headers["content-type"] !== mediaType) {
      return new UnsupportedMediaTypeFailure(
        `Unsupported Media Type, expected ${mediaType}`,
      )
    }
  }
