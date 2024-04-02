export type RequestContext<T extends object = object> = {
  url: URL
  request: Request
  headers: Headers
} & T

export function createContext(request: Request): RequestContext {
  let url: URL | undefined
  const context = {
    get url() {
      if (url) return url
      url = new URL(request.url)
      return url
    },
    request,
    headers: new Headers(),
  }

  return context
}
