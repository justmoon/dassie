import { ServerResponse } from "node:http"

import {
  DEFAULT_HTTP_RESPONSE_OPTIONS,
  DefaultHttpResponse,
  HttpResponseOptions,
} from "./default-http-reponse"

export const DEFAULT_BINARY_OPTIONS = {
  ...DEFAULT_HTTP_RESPONSE_OPTIONS,
  contentType: "application/octet-stream",
} as const satisfies HttpResponseOptions

export class BinaryHttpResponse extends DefaultHttpResponse {
  constructor(
    readonly data: Uint8Array,
    options: Partial<HttpResponseOptions>,
  ) {
    super({
      ...DEFAULT_BINARY_OPTIONS,
      ...options,
    })
  }

  override applyTo(response: ServerResponse) {
    super.applyStatusAndHeader(response)
    response.end(this.data)
  }
}

export const createBinaryResponse = (
  data: Uint8Array,
  options: Partial<HttpResponseOptions>,
): BinaryHttpResponse => new BinaryHttpResponse(data, options)
