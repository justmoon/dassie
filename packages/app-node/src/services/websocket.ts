import type { WebSocket } from "ws"
import { WebSocketServer } from "ws"

import type { Config } from "../config"
import type HttpService from "./http"

interface WebSocketContext {
  config: Config
  http: HttpService
}

export default class WebSocketService {
  readonly ctx: WebSocketContext
  readonly wss: WebSocketServer

  constructor(context: WebSocketContext) {
    this.ctx = context
    this.wss = new WebSocketServer({ server: this.ctx.http.server })

    this.wss.on("connection", this.handleConnection.bind(this))
  }

  handleConnection(socket: WebSocket) {
    console.log("handle socket")

    socket.send("hello")
  }
}
