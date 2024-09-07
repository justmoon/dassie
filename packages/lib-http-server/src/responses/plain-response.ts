import type { BaseRequestContext } from "../types/context"
import {
  DEFAULT_HTTP_RESPONSE_OPTIONS,
  DefaultHttpResponse,
  type HttpResponseOptions,
} from "./default-http-reponse"

export const DEFAULT_PLAIN_OPTIONS = {
  ...DEFAULT_HTTP_RESPONSE_OPTIONS,
  contentType: "text/plain",
} as const satisfies HttpResponseOptions

export class PlainHttpResponse extends DefaultHttpResponse {
  constructor(
    readonly data: string,
    options: Partial<HttpResponseOptions> = {},
  ) {
    super({
      ...DEFAULT_PLAIN_OPTIONS,
      ...options,
    })
  }

  override asResponse(context: BaseRequestContext) {
    return new Response(this.data, this.getStatusAndHeaders(context))
  }
}

export const createPlainResponse = (
  data: string,
  options: Partial<HttpResponseOptions>,
): PlainHttpResponse => new PlainHttpResponse(data, options)
