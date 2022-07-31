import axios from "axios"

import { createLogger } from "@xen-ilp/lib-logger"
import { EffectContext, createTopic } from "@xen-ilp/lib-reactive"

import { peerTableStore } from "./stores/peer-table"

const logger = createLogger("xen:node:outgoing-xen-message-sender")

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
        accept: "application/xen-peer-message",
        "content-type": "application/xen-peer-message",
      },
    })
  }

  sig.onAsync(outgoingPeerMessageBufferTopic, sendOutgoingPeerMessage)
}
