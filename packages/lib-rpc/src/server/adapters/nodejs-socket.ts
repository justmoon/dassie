import { createTopic } from "@dassie/lib-reactive"

import type { NodejsSocket } from "../../common/types/nodejs-socket"
import type { InboundConnection } from "../server"

export function createNodejsSocketAdapter(
  nodejsSocket: NodejsSocket,
): InboundConnection {
  const received = createTopic<string>()

  nodejsSocket.setEncoding("utf8")

  const buffer: string[] = []

  nodejsSocket.on("data", (data) => {
    while (data.length > 0) {
      const endOfLine = data.indexOf("\n")
      if (endOfLine === -1) {
        buffer.push(data)
        return
      }

      const message = "".concat(...buffer, data.slice(0, endOfLine))
      buffer.length = 0

      received.emit(message)

      data = data.slice(endOfLine + 1)
    }
  })

  const closed = new Promise<void>((resolve) => {
    nodejsSocket.on("close", resolve)
  })

  function send(data: string) {
    nodejsSocket.write(data + "\n")
  }

  return {
    received,
    send,
    closed,
  }
}
