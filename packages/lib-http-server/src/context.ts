import type { BaseRequestContext } from "./types/context"

export function createContext(request: Request): BaseRequestContext {
  let url: URL | undefined
  const context = {
    get url() {
      if (url) return url
      url = new URL(request.url)
      return url
    },
    request,
    response: {
      status: undefined,
      statusText: undefined,
      headers: new Headers(),
    },
  }

  return context
}
