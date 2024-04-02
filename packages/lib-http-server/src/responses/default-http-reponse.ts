import type { RequestContext } from "../context"
import { HttpResponse } from "../types/http-response"

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

  getStatusAndHeaders({ headers }: RequestContext) {
    if (!headers.has("Content-Type")) {
      headers.set("Content-Type", this.options.contentType)
    }

    return {
      status: this.options.statusCode,
      headers,
    } satisfies ResponseInit
  }
}
