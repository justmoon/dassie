import type { BaseRequestContext } from "../types/context"

export function getResponseOptionsFromContext(
  context: BaseRequestContext,
): ResponseInit {
  const responseInit: ResponseInit = {
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
