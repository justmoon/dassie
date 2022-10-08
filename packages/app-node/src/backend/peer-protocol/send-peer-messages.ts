import axios from "axios"

import { createLogger } from "@dassie/lib-logger"
import { EffectContext, createTopic } from "@dassie/lib-reactive"

import { configSignal } from "../config"
import { peerMessage } from "./peer-schema"
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
  const nodeId = sig.get(configSignal, (config) => config.nodeId)

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

    const envelopeSerializationResult = peerMessage.serialize({
      version: 0,
      sender: nodeId,
      authentication: {
        "ED25519_X25519_HMAC-SHA256": {
          sessionPublicKey: Uint8Array.from(Buffer.alloc(32)),
        },
      },
      content: { bytes: message },
    })

    if (!envelopeSerializationResult.success) {
      logger.warn("failed to serialize message", {
        error: envelopeSerializationResult.failure,
      })
      return
    }

    await axios(`${peer.url}/peer`, {
      method: "POST",
      data: envelopeSerializationResult.value,
      headers: {
        accept: "application/dassie-peer-message",
        "content-type": "application/dassie-peer-message",
      },
    })
  }

  sig.onAsync(outgoingPeerMessageBufferTopic, sendOutgoingPeerMessage)
}
