import { Server, createServer } from "node:https"

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
      (request, response) => {
        const url = new URL(request.url!, `https://${request.headers.host}`)

        switch (url.pathname) {
          case "/":
            response.writeHead(200, { "Content-Type": "text/html" })
            response.end(`Hello World!`)
        }
        console.log(url)
      }
    )

    this.server

    this.server.listen(port)

    console.log(
      `listening on https://${context.config.host}${
        port === 443 ? "" : `:${port}`
      }/`
    )
  }
}
