import type { BaseRequestContext } from "../types/context"

export interface ResponseOptions {
  headers: Headers
  status?: number
  statusText?: string
}

export function getResponseOptionsFromContext(
  context: BaseRequestContext,
): ResponseOptions {
  const responseInit: ResponseOptions = {
    headers: context.response.headers,
  }

  if (context.response.status) {
    responseInit.status = context.response.status
  }

  if (context.response.statusText) {
    responseInit.statusText = context.response.statusText
  }

  return responseInit
}
