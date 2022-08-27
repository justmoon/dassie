import type { ServerResponse } from "node:http"

export const respondPlainly = (
  response: ServerResponse,
  statusCode: number,
  message: string
) => {
  response.writeHead(statusCode, { "Content-Type": "text/plain" })
  response.end(message)
}
