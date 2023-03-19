import axios from "axios"
import type { SetOptional } from "type-fest"

import { createLogger } from "@dassie/lib-logger"
import type { InferSerialize } from "@dassie/lib-oer"
import { Reactor, createTopic } from "@dassie/lib-reactive"

import { configSignal } from "../../config"
import { peerMessage, peerMessageContent } from "../peer-schema"
import { nodeTableStore } from "../stores/node-table"

const logger = createLogger("das:node:outgoing-dassie-message-sender")

export type MessageWithDestination = SetOptional<
  OutgoingPeerMessageEvent,
  "asUint8Array"
>

export interface OutgoingPeerMessageEvent {
  message: InferSerialize<typeof peerMessageContent>
  subnet: string
  destination: string
  asUint8Array: Uint8Array
}

export const outgoingPeerMessageTopic = () =>
  createTopic<OutgoingPeerMessageEvent>()

const serializePeerMessage = (
  message: InferSerialize<typeof peerMessageContent>
) => {
  const messageSerializeResult = peerMessageContent.serialize(message)

  if (!messageSerializeResult.success) {
    throw new Error("Failed to serialize peer message", {
      cause: messageSerializeResult.error,
    })
  }

  return messageSerializeResult.value
}

export const sendPeerMessage = (reactor: Reactor) => {
  return async (parameters: MessageWithDestination) => {
    const peers = reactor.use(nodeTableStore).read()
    const { nodeId } = reactor.use(configSignal).read()
    const { message, subnet, destination, asUint8Array } = parameters

    const serializedMessage = asUint8Array ?? serializePeerMessage(message)

    reactor.use(outgoingPeerMessageTopic).emit({
      ...parameters,
      asUint8Array: serializedMessage,
    })

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
        type: "ED25519_X25519_HMAC-SHA256",
        value: {
          sessionPublicKey: Uint8Array.from(Buffer.alloc(32)),
        },
      },
      content: { bytes: serializedMessage },
    })

    if (!envelopeSerializationResult.success) {
      logger.warn("failed to serialize message", {
        error: envelopeSerializationResult.error,
      })
      return
    }

    const result = await axios<Uint8Array>(`${peer.url}/peer`, {
      method: "POST",
      data: envelopeSerializationResult.value,
      headers: {
        accept: "application/dassie-peer-message",
        "content-type": "application/dassie-peer-message",
      },
      responseType: "arraybuffer",
    })

    return result.data
  }
}
