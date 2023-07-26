import axios from "axios"
import type { SetOptional } from "type-fest"

import type { InferSerialize } from "@dassie/lib-oer"
import { createActor, createTopic } from "@dassie/lib-reactive"

import { nodeIdSignal } from "../../ilp-connector/computed/node-id"
import { peerProtocol as logger } from "../../logger/instances"
import { peerMessage, peerMessageContent } from "../peer-schema"
import { nodeTableStore } from "../stores/node-table"
import { NodeId } from "../types/node-id"

export type MessageWithDestination = SetOptional<
  OutgoingPeerMessageEvent,
  "asUint8Array"
>

export interface OutgoingPeerMessageEvent {
  message: InferSerialize<typeof peerMessageContent>
  destination: NodeId
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

export const sendPeerMessage = () =>
  createActor((sig) => {
    const nodeId = sig.get(nodeIdSignal)
    const peers = sig.use(nodeTableStore)

    return {
      send: async (parameters: MessageWithDestination) => {
        const { message, destination, asUint8Array } = parameters

        const serializedMessage = asUint8Array ?? serializePeerMessage(message)

        sig.use(outgoingPeerMessageTopic).emit({
          ...parameters,
          asUint8Array: serializedMessage,
        })

        const peer = peers.read().get(destination)

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

        try {
          const result = await axios<Buffer>(`${peer.url}/peer`, {
            method: "POST",
            data: envelopeSerializationResult.value,
            headers: {
              accept: "application/dassie-peer-message",
              "content-type": "application/dassie-peer-message",
            },
            responseType: "arraybuffer",
          })

          return new Uint8Array(
            result.data.buffer,
            result.data.byteOffset,
            result.data.byteLength
          )
        } catch (error) {
          logger.warn("failed to send message", {
            error,
            url: peer.url,
          })

          return
        }
      },
    }
  })
