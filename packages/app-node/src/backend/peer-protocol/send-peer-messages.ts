import axios from "axios"

import { createLogger } from "@dassie/lib-logger"
import { EffectContext, createTopic } from "@dassie/lib-reactive"

import { configSignal } from "../config"
import { peerMessage } from "./peer-schema"
import { peerTableStore } from "./stores/peer-table"

const logger = createLogger("das:node:outgoing-dassie-message-sender")

export interface MessageWithDestination {
  message: Uint8Array
  subnet: string
  destination: string
}

export const outgoingPeerMessageBufferTopic = () =>
  createTopic<MessageWithDestination>()

export const sendPeerMessage = (sig: EffectContext) => {
  const { reactor } = sig
  const peers = sig.get(peerTableStore)
  const { nodeId } = sig.getKeys(configSignal, ["nodeId"])

  return (parameters: MessageWithDestination) => {
    const { message, subnet, destination } = parameters
    reactor.use(outgoingPeerMessageBufferTopic).emit(parameters)

    const peer = peers.get(`${subnet}.${destination}`)

    if (!peer) {
      logger.warn("peer not found, unable to send message", {
        to: destination,
      })
      return
    }

    const envelopeSerializationResult = peerMessage.serialize({
      version: 0,
      subnetId: subnet,
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
        error: envelopeSerializationResult.error,
      })
      return
    }

    axios(`${peer.url}/peer`, {
      method: "POST",
      data: envelopeSerializationResult.value,
      headers: {
        accept: "application/dassie-peer-message",
        "content-type": "application/dassie-peer-message",
      },
    }).catch((error: unknown) => {
      logger.warn("failed to send message", {
        error,
      })
    })
  }
}
