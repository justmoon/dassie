import { ServerResponse } from "node:http"

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

  abstract applyTo(response: ServerResponse): void

  applyStatusAndHeader(response: ServerResponse) {
    response.statusCode = this.options.statusCode
    response.setHeader("Content-Type", this.options.contentType)
  }
}
