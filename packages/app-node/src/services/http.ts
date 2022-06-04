import type { IncomingMessage, ServerResponse } from "node:http"
import { Server, createServer } from "node:https"

import { assertDefined } from "@xen-ilp/lib-type-utils"

import type { Config } from "../config"

export interface HttpContext {
  config: Config
}

export default class HttpService {
  readonly ctx: HttpContext
  readonly server: Server

  constructor(context: HttpContext) {
    this.ctx = context

    const port = context.config.port

    this.server = createServer(
      {
        cert: context.config.tlsCert,
        key: context.config.tlsKey,
      },
      this.handleRequest
    )

    this.server.listen(port)

    console.log(
      `listening on https://${context.config.host}${
        port === 443 ? "" : `:${port}`
      }/`
    )
  }

  handleRequest = (request: IncomingMessage, response: ServerResponse) => {
    try {
      assertDefined(request.url)
      assertDefined(request.method)

      const routes: Record<string, Record<string, () => void>> = {
        GET: {
          "/": () => {
            response.writeHead(200, { "Content-Type": "text/html" })
            response.end(`Hello World!`)
          },
        },
        POST: {
          "/ilp": () => {
            if (request.headers["accept"] !== "application/octet-stream") {
              response.writeHead(400, { "Content-Type": "text/plain" })
              response.end(`Expected application/octet-stream`)
              return
            }
            if (
              request.headers["content-type"] !== "application/octet-stream"
            ) {
              response.writeHead(400, { "Content-Type": "text/plain" })
              response.end(`Expected application/octet-stream`)
              return
            }

            response.writeHead(400, { "Content-Type": "text/plain" })
            response.end("not yet implemented")
          },
          "/xen": () => {
            if (request.headers["accept"] !== "application/json") {
              response.writeHead(400, { "Content-Type": "text/plain" })
              response.end(`Expected application/json`)
              return
            }
            if (request.headers["content-type"] !== "application/json") {
              response.writeHead(400, { "Content-Type": "text/plain" })
              response.end(`Expected application/octet-stream`)
              return
            }

            response.writeHead(400, { "Content-Type": "text/plain" })
            response.end("not yet implemented")
          },
        },
      }

      const send404 = () => {
        response.writeHead(404, { "Content-Type": "text/html" })
        response.end(`<h1>404</h1>`)
      }

      // Call route handler or 404 if no matching route exists
      ;(routes[request.method]?.[request.url] ?? send404)()
    } catch (error) {
      console.error(error)
      response.writeHead(500, { "Content-Type": "text/html" })
      response.end(`<h1>500</h1>`)
    }
  }
}
