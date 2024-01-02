import { Middleware } from "./router"

export const url: Middleware<object, { parsedUrl: URL }> = (request) => {
  const url = new URL(request.url ?? "", `http://${request.headers.host}`)
  return { parsedUrl: url }
}
