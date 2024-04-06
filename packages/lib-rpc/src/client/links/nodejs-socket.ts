import { createTopic } from "@dassie/lib-reactive"

import type { NodejsSocket } from "../../common/types/nodejs-socket"
import type { OutboundConnection } from "../client"

export function createNodejsSocketLink(
  nodejsSocket: NodejsSocket,
): OutboundConnection {
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

  function send(data: string) {
    nodejsSocket.write(data + "\n")
  }

  function close() {
    nodejsSocket.end()
  }

  return {
    received,
    send,
    close,
  }
}
