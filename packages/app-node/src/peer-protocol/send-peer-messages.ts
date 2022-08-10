import { createLogger } from "@dassie/lib-logger"
import { EffectContext, createTopic } from "@dassie/lib-reactive"
import axios from "axios"

import { peerTableStore } from "./stores/peer-table"

const logger = createLogger("das:node:outgoing-dassie-message-sender")

export interface MessageWithDestination<T> {
  message: T
  destination: string
}

export const outgoingPeerMessageBufferTopic = () =>
  createTopic<MessageWithDestination<Uint8Array>>()

export const sendPeerMessages = (sig: EffectContext) => {
  const peers = sig.get(peerTableStore)

  const sendOutgoingPeerMessage = async ({
    message,
    destination,
  }: MessageWithDestination<Uint8Array>) => {
    const peer = peers.get(destination)

    if (!peer) {
      logger.warn("peer not found, unable to send message", {
        to: destination,
      })
      return
    }

    await axios(`${peer.url}/peer`, {
      method: "POST",
      data: message,
      headers: {
        accept: "application/dassie-peer-message",
        "content-type": "application/dassie-peer-message",
      },
    })
  }

  sig.onAsync(outgoingPeerMessageBufferTopic, sendOutgoingPeerMessage)
}
