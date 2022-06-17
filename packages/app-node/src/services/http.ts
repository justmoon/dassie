import type { IncomingMessage, ServerResponse } from "node:http"
import { Server, createServer } from "node:https"

import { createLogger } from "@xen-ilp/lib-logger"
import { assertDefined } from "@xen-ilp/lib-type-utils"

import type { Config } from "../config"
import { MAX_BODY_SIZE } from "../constants/http"
import { parseMessage } from "../protocols/xen/message"
import type PeerManager from "./peer-manager"

const logger = createLogger("xen:node:http")

export interface HttpContext {
  config: Config
  peerManager: PeerManager
}

export default class HttpService {
  readonly server: Server

  constructor(readonly context: HttpContext) {
    this.context = context

    const port = context.config.port

    this.server = createServer(
      {
        cert: context.config.tlsWebCert,
        key: context.config.tlsWebKey,
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

      const routes: Record<
        string,
        Record<
          string,
          (request: IncomingMessage, response: ServerResponse) => void
        >
      > = {
        GET: {
          "/": this.handleGetRoot,
        },
        POST: {
          "/ilp": this.handlePostIlp,
          "/xen": this.handlePostXen,
        },
      }

      const send404 = () => {
        response.writeHead(404, { "Content-Type": "text/html" })
        response.end(`<h1>404</h1>`)
      }

      // Call route handler or 404 if no matching route exists
      ;(routes[request.method]?.[request.url] ?? send404)(request, response)
    } catch (error) {
      console.error(error)
      response.writeHead(500, { "Content-Type": "text/html" })
      response.end(`<h1>500</h1>`)
    }
  }

  handleGetRoot = (_request: IncomingMessage, response: ServerResponse) => {
    response.writeHead(200, { "Content-Type": "text/html" })
    response.end(`Hello World!`)
  }

  handlePostIlp = (request: IncomingMessage, response: ServerResponse) => {
    if (request.headers["accept"] !== "application/octet-stream") {
      response.writeHead(400, { "Content-Type": "text/plain" })
      response.end(`Expected application/octet-stream`)
      return
    }
    if (request.headers["content-type"] !== "application/octet-stream") {
      response.writeHead(400, { "Content-Type": "text/plain" })
      response.end(`Expected application/octet-stream`)
      return
    }

    response.writeHead(400, { "Content-Type": "text/plain" })
    response.end("not yet implemented")
  }

  handlePostXen = (request: IncomingMessage, response: ServerResponse) => {
    if (
      !request.headers["accept"] ||
      request.headers["accept"].indexOf("application/xen-message") === -1
    ) {
      response.writeHead(400, { "Content-Type": "text/plain" })

      logger.debug("invalid accept header", {
        accept: request.headers["accept"],
      })
      response.end(`Expected application/xen-message in accept header`)
      return
    }
    if (request.headers["content-type"] !== "application/xen-message") {
      response.writeHead(400, { "Content-Type": "text/plain" })
      response.end(`Expected application/xen-message in content-type header`)
      return
    }

    const body: Array<Buffer> = []
    let dataReceived: number = 0

    request.on("data", (chunk) => {
      dataReceived += chunk.length

      if (dataReceived > MAX_BODY_SIZE) {
        response.writeHead(400, { "Content-Type": "text/plain" })
        response.end(`Expected body to be less than ${MAX_BODY_SIZE} bytes`)
        return
      }
      body.push(chunk)
    })

    request.on("end", () => {
      try {
        const message = parseMessage(Buffer.concat(body))
        this.context.peerManager.handleMessage(message)
      } catch (error) {
        console.log(`received invalid message: ${error}`)
        response.writeHead(400, { "Content-Type": "application/json" })
        response.end(JSON.stringify({ error: "invalid message" }))
      }
    })
  }
}
