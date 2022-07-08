import axios from "axios"

import { createLogger } from "@xen-ilp/lib-logger"
import type { EffectContext } from "@xen-ilp/lib-reactive"

import { peerTableStore } from "../peering/stores/peer-table"
import { outgoingXenMessageBufferTopic } from "./topics/xen-protocol"

const logger = createLogger("xen:node:outgoing-xen-message-sender")

export const outgoingXenMessageSender = (sig: EffectContext) => {
  const peers = sig.get(peerTableStore)

  const sendOutgoingXenMessage = async ({
    message,
    destination,
  }: {
    message: Buffer
    destination: string
  }) => {
    const peer = peers[destination]

    if (!peer) {
      logger.warn("peer not found, unable to send message", {
        to: destination,
      })
      return
    }

    await axios(`${peer.url}/xen`, {
      method: "POST",
      data: message,
      headers: {
        accept: "application/xen-message",
        "content-type": "application/xen-message",
      },
    })
  }

  sig.on(outgoingXenMessageBufferTopic, sendOutgoingXenMessage)
}
