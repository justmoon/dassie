import { createTopic } from "@dassie/lib-reactive"

import type { NodejsMessagePort } from "../../common/types/nodejs-messageport"
import type { OutboundConnection } from "../client"

export function createNodejsMessagePortLink(
  messagePort: NodejsMessagePort,
): OutboundConnection {
  const received = createTopic<string>()

  function handleMessage(message: unknown) {
    if (typeof message !== "string") {
      console.warn("Unexpected message type", { type: typeof message })
      return
    }

    received.emit(message)
  }
  messagePort.on("message", handleMessage)

  function send(data: string) {
    messagePort.postMessage(data)
  }

  function close() {
    messagePort.off("message", handleMessage)
    messagePort.close()
  }

  return {
    received,
    send,
    close,
  }
}
