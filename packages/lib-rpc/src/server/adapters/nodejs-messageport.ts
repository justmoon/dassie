import { createTopic } from "@dassie/lib-reactive"

import type { NodejsMessagePort } from "../../common/types/nodejs-messageport"
import type { InboundConnection } from "../server"

export function createNodejsMessagePortAdapter(
  messagePort: NodejsMessagePort,
): InboundConnection {
  const received = createTopic<string>()

  messagePort.on("message", (message: unknown) => {
    if (typeof message !== "string") {
      throw new TypeError("Unexpected message type")
    }

    received.emit(message)
  })

  messagePort.on("error", (error: unknown) => {
    console.warn("inbound error; closing message port", { error })
    messagePort.close()
  })

  const closed = new Promise<void>((resolve) => {
    messagePort.on("close", resolve)
  })

  function send(data: string) {
    messagePort.postMessage(data)
  }

  return {
    received,
    send,
    closed,
  }
}
