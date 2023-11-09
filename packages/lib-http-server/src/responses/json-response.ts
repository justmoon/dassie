import type { Response } from "express"
import { JsonValue } from "type-fest"

import {
  DEFAULT_HTTP_RESPONSE_OPTIONS,
  DefaultHttpResponse,
  HttpResponseOptions,
} from "./default-http-reponse"

const DEFAULT_JSON_OPTIONS = {
  ...DEFAULT_HTTP_RESPONSE_OPTIONS,
  contentType: "application/json",
} as const satisfies HttpResponseOptions

export class JsonHttpResponse extends DefaultHttpResponse {
  constructor(
    readonly data: JsonValue,
    options: Partial<HttpResponseOptions>,
  ) {
    super({
      ...DEFAULT_JSON_OPTIONS,
      ...options,
    })
  }

  override applyTo(response: Response) {
    super.applyTo(response)
    response.end(JSON.stringify(this.data))
  }
}

export const createJsonResponse = (
  data: JsonValue,
  options: Partial<HttpResponseOptions> = {},
): JsonHttpResponse => new JsonHttpResponse(data, options)
