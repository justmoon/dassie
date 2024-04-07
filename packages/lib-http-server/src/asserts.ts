import { NotAcceptableFailure, UnsupportedMediaTypeFailure } from "."
import type { RequestContext } from "./types/context"

export const createAcceptHeaderAssertion =
  (mediaType: string) =>
  ({ request }: RequestContext): NotAcceptableFailure | void => {
    if (request.headers.get("accept") !== mediaType) {
      return new NotAcceptableFailure(`Not Acceptable, expected ${mediaType}`)
    }
  }

export const createContentTypeHeaderAssertion =
  (mediaType: string) =>
  ({ request }: RequestContext): UnsupportedMediaTypeFailure | void => {
    if (request.headers.get("content-type") !== mediaType) {
      return new UnsupportedMediaTypeFailure(
        `Unsupported Media Type, expected ${mediaType}`,
      )
    }
  }
