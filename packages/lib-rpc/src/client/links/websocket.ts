import { createTopic } from "@dassie/lib-reactive"

import type { WebSocketImplementation } from "../../common/types/websocket"
import { WebSocketConstructor } from "../../common/types/websocket"
import {
  type ExponentialBackoffParameters,
  createExponentialBackoffCalculator,
} from "../../common/utils/exponential-backoff"
import type { OutboundConnection } from "../client"

export interface WebSocketLinkOptions {
  url: string
  WebSocket?: WebSocketConstructor | undefined
  reconnectDelay?:
    | number
    | ((attempt: number) => number)
    | ExponentialBackoffParameters
    | undefined
}

const STATE_OPEN = 1

export function createWebSocketLink({
  url,
  WebSocket = globalThis.WebSocket,
  reconnectDelay,
}: WebSocketLinkOptions): OutboundConnection {
  const received = createTopic<string>()
  const resets = createTopic<void>()

  const calculateExponentialBackoff =
    typeof reconnectDelay === "number"
      ? () => reconnectDelay
      : typeof reconnectDelay === "function"
        ? reconnectDelay
        : createExponentialBackoffCalculator(reconnectDelay)

  let currentSocket: WebSocketImplementation | undefined
  const sendQueue: string[] = []
  let attempt = 0
  let isClosed = false

  function connect() {
    if (isClosed) return

    const socket = new WebSocket(url)
    currentSocket = socket

    socket.addEventListener("message", ({ data }) => {
      if (isClosed) return

      if (typeof data !== "string") {
        throw new TypeError("Expected string data")
      }

      received.emit(data)
    })

    socket.addEventListener("open", () => {
      if (isClosed) return

      attempt = 0

      for (const data of sendQueue) {
        socket.send(data)
      }
      sendQueue.length = 0
    })

    socket.addEventListener("close", () => {
      if (isClosed) return

      resets.emit()
      currentSocket = undefined

      const delay = calculateExponentialBackoff(attempt++)
      setTimeout(connect, delay)
    })
  }

  function send(data: string) {
    if (isClosed) return

    if (currentSocket?.readyState === STATE_OPEN) {
      currentSocket.send(data)
    } else {
      sendQueue.push(data)
    }
  }

  function close() {
    isClosed = true
    currentSocket?.close()
  }

  connect()

  return {
    received,
    send,
    resets,
    close,
  }
}
