import type { JsonValue } from "type-fest"

import type { BaseRequestContext } from "../types/context"
import {
  DEFAULT_HTTP_RESPONSE_OPTIONS,
  DefaultHttpResponse,
  type HttpResponseOptions,
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

  override asResponse(context: BaseRequestContext) {
    return new Response(
      JSON.stringify(this.data),
      this.getStatusAndHeaders(context),
    )
  }
}

export const createJsonResponse = (
  data: JsonValue,
  options: Partial<HttpResponseOptions> = {},
): JsonHttpResponse => new JsonHttpResponse(data, options)
