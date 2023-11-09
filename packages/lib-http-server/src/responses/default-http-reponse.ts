import type { Response } from "express"

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

  applyTo(response: Response) {
    response.status(this.options.statusCode).type(this.options.contentType)
  }
}
