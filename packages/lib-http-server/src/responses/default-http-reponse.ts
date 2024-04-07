import type { RequestContext } from "../types/context"
import { HttpResponse } from "../types/http-response"
import { getResponseOptionsFromContext } from "../utils/get-response-from-context"

export interface HttpResponseOptions {
  statusCode: number
  contentType: string
}

export const DEFAULT_HTTP_RESPONSE_OPTIONS = {
  statusCode: 200,
  contentType: "text/plain",
} as const

export abstract class DefaultHttpResponse implements HttpResponse {
  constructor(private readonly options: HttpResponseOptions) {}

  abstract asResponse(context: RequestContext): Response

  getStatusAndHeaders(context: RequestContext) {
    const { response } = context
    if (!response.headers.has("Content-Type")) {
      response.headers.set("Content-Type", this.options.contentType)
    }

    const responseInit = getResponseOptionsFromContext(context)

    return responseInit
  }
}
