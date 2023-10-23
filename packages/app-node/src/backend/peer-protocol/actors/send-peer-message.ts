import axios from "axios"
import type { SetOptional } from "type-fest"

import type { InferSerialize } from "@dassie/lib-oer"
import { createActor, createTopic } from "@dassie/lib-reactive"

import { EnvironmentConfigSignal } from "../../config/environment-config"
import { NodeIdSignal } from "../../ilp-connector/computed/node-id"
import { peerProtocol as logger } from "../../logger/instances"
import { bufferToUint8Array } from "../../utils/buffer-to-typedarray"
import { peerMessage, peerMessageContent } from "../peer-schema"
import { NodeTableStore } from "../stores/node-table"
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

export const OutgoingPeerMessageTopic = () =>
  createTopic<OutgoingPeerMessageEvent>()

const serializePeerMessage = (
  message: InferSerialize<typeof peerMessageContent>,
) => {
  const messageSerializeResult = peerMessageContent.serialize(message)

  if (!messageSerializeResult.success) {
    throw new Error("Failed to serialize peer message", {
      cause: messageSerializeResult.error,
    })
  }

  return messageSerializeResult.value
}

interface NodeContactInfo {
  url: string
  publicKey: Uint8Array
}

export const SendPeerMessageActor = () =>
  createActor((sig) => {
    const nodeId = sig.get(NodeIdSignal)
    const nodeTable = sig.use(NodeTableStore)
    const { bootstrapNodes } = sig.getKeys(EnvironmentConfigSignal, [
      "bootstrapNodes",
    ])

    const getNodeContactInfo = (
      nodeId: NodeId,
    ): NodeContactInfo | undefined => {
      // If the node is in the node table, we can use the URL from the link state
      {
        const node = nodeTable.read().get(nodeId)

        if (node?.linkState) {
          return {
            url: node.linkState.url,
            publicKey: node.linkState.publicKey,
          }
        }
      }

      // If the node is a bootstrap node, we can use the URL from the config
      {
        const node = bootstrapNodes.find((node) => node.id === nodeId)

        if (node) {
          return {
            url: node.url,
            publicKey: bufferToUint8Array(
              Buffer.from(node.publicKey, "base64url"),
            ),
          }
        }
      }

      return undefined
    }

    return sig.handlers({
      send: async (parameters: MessageWithDestination) => {
        const { message, destination, asUint8Array } = parameters

        const serializedMessage = asUint8Array ?? serializePeerMessage(message)

        sig.use(OutgoingPeerMessageTopic).emit({
          ...parameters,
          asUint8Array: serializedMessage,
        })

        const contactInfo = getNodeContactInfo(destination)

        if (!contactInfo) {
          logger.warn("no url known for destination, unable to send message", {
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
          const result = await axios<Buffer>(`${contactInfo.url}/peer`, {
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
            result.data.byteLength,
          )
        } catch (error) {
          logger.warn("failed to send message", {
            error,
            to: destination,
            url: contactInfo.url,
          })

          return
        }
      },
    })
  })
