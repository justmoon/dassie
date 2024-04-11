import { NotAcceptableFailure, UnsupportedMediaTypeFailure } from "."
import type { BaseRequestContext } from "./types/context"

export const createAcceptHeaderAssertion =
  (mediaType: string) =>
  ({ request }: BaseRequestContext): NotAcceptableFailure | void => {
    if (request.headers.get("accept") !== mediaType) {
      return new NotAcceptableFailure(`Not Acceptable, expected ${mediaType}`)
    }
  }

export const createContentTypeHeaderAssertion =
  (mediaType: string) =>
  ({ request }: BaseRequestContext): UnsupportedMediaTypeFailure | void => {
    if (request.headers.get("content-type") !== mediaType) {
      return new UnsupportedMediaTypeFailure(
        `Unsupported Media Type, expected ${mediaType}`,
      )
    }
  }
