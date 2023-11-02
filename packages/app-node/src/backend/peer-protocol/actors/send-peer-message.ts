import axios from "axios"
import type { SetOptional } from "type-fest"

import type { Infer, InferSerialize } from "@dassie/lib-oer"
import { createActor, createTopic } from "@dassie/lib-reactive"

import { EnvironmentConfigSignal } from "../../config/environment-config"
import { NodeIdSignal } from "../../ilp-connector/computed/node-id"
import { peerProtocol as logger } from "../../logger/instances"
import { bufferToUint8Array } from "../../utils/buffer-to-typedarray"
import { DASSIE_MESSAGE_CONTENT_TYPE } from "../constants/content-type"
import { DEFAULT_NODE_COMMUNICATION_TIMEOUT } from "../constants/timings"
import { GenerateMessageAuthentication } from "../functions/generate-message-authentication"
import {
  peerMessage,
  peerMessageContent,
  peerMessageResponse,
} from "../peer-schema"
import { NodeTableStore } from "../stores/node-table"
import { NodeId } from "../types/node-id"
import { PeerMessageType } from "./handle-peer-message"

export type SendPeerMessageParameters<TMessageType extends PeerMessageType> =
  SetOptional<OutgoingPeerMessageEvent, "asUint8Array"> & {
    message: { type: TMessageType }
    timeout?: number | undefined
  }

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
    const generateMessageAuthentication = sig.use(GenerateMessageAuthentication)

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

    return {
      send: async <const TMessageType extends PeerMessageType>(
        parameters: SendPeerMessageParameters<TMessageType>,
      ): Promise<
        Infer<(typeof peerMessageResponse)[TMessageType]> | undefined
      > => {
        const { message, destination, asUint8Array, timeout } = parameters

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

        const authentication = generateMessageAuthentication(
          serializedMessage,
          message.type,
          destination,
          contactInfo.publicKey,
        )

        const envelopeSerializationResult = peerMessage.serialize({
          version: 0,
          sender: nodeId,
          authentication,
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
              accept: DASSIE_MESSAGE_CONTENT_TYPE,
              "content-type": DASSIE_MESSAGE_CONTENT_TYPE,
            },
            responseType: "arraybuffer",
            timeout: timeout ?? DEFAULT_NODE_COMMUNICATION_TIMEOUT,
          })

          const resultUint8Array = new Uint8Array(
            result.data.buffer,
            result.data.byteOffset,
            result.data.byteLength,
          )

          const responseSchema = peerMessageResponse[message.type]

          const response = responseSchema.parse(resultUint8Array)

          if (!response.success) {
            logger.warn("failed to parse response", {
              error: response.error,
              from: destination,
            })
            return
          }

          return response.value as Infer<typeof responseSchema>
        } catch (error) {
          logger.warn("failed to send message", {
            error,
            to: destination,
            url: contactInfo.url,
          })

          return
        }
      },
    }
  })
