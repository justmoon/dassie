import axios from "axios"
import type { SetOptional } from "type-fest"

import type { Infer, InferSerialize } from "@dassie/lib-oer"
import { Reactor, createTopic } from "@dassie/lib-reactive"
import {
  bufferToUint8Array,
  isErrorWithCode,
  isFailure,
} from "@dassie/lib-type-utils"

import { EnvironmentConfig } from "../../config/environment-config"
import { NodeIdSignal } from "../../ilp-connector/computed/node-id"
import { peerProtocol as logger } from "../../logger/instances"
import { DASSIE_MESSAGE_CONTENT_TYPE } from "../constants/content-type"
import { DEFAULT_NODE_COMMUNICATION_TIMEOUT } from "../constants/timings"
import {
  peerMessage,
  peerMessageContent,
  peerMessageResponse,
} from "../peer-schema"
import { NodeTableStore } from "../stores/node-table"
import { NodeId } from "../types/node-id"
import { GenerateMessageAuthentication } from "./generate-message-authentication"
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
  const messageSerializeResult = peerMessageContent.serializeOrThrow(message)

  return messageSerializeResult
}

interface NodeContactInfo {
  url: string
  publicKey: Uint8Array
}

export const SendPeerMessage = (reactor: Reactor) => {
  const nodeIdSignal = reactor.use(NodeIdSignal)
  const nodeTable = reactor.use(NodeTableStore)
  const environmentConfig = reactor.use(EnvironmentConfig)
  const generateMessageAuthentication = reactor.use(
    GenerateMessageAuthentication,
  )
  const outgoingPeerMessageTopic = reactor.use(OutgoingPeerMessageTopic)

  const getNodeContactInfo = (nodeId: NodeId): NodeContactInfo | undefined => {
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
      const node = environmentConfig.bootstrapNodes.find(
        (node) => node.id === nodeId,
      )

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

  async function sendPeerMessage<const TMessageType extends PeerMessageType>(
    parameters: SendPeerMessageParameters<TMessageType>,
  ): Promise<Infer<(typeof peerMessageResponse)[TMessageType]> | undefined> {
    const { message, destination, asUint8Array, timeout } = parameters

    const serializedMessage = asUint8Array ?? serializePeerMessage(message)

    outgoingPeerMessageTopic.emit({
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
      sender: nodeIdSignal.read(),
      authentication,
      content: { bytes: serializedMessage },
    })

    if (isFailure(envelopeSerializationResult)) {
      logger.warn("failed to serialize message", {
        error: envelopeSerializationResult,
      })
      return
    }

    try {
      const result = await axios<Buffer>(`${contactInfo.url}/peer`, {
        method: "POST",
        data: envelopeSerializationResult,
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

      if (isFailure(response)) {
        logger.warn("failed to parse response", {
          error: response,
          from: destination,
        })
        return
      }

      return response.value as Infer<typeof responseSchema>
    } catch (error) {
      if (isErrorWithCode(error, "ECONNREFUSED")) {
        logger.warn(
          "failed to send message, connection refused, the node may be offline",
          {
            to: destination,
            url: contactInfo.url,
          },
        )
        return
      }

      logger.warn("failed to send message", {
        error,
        to: destination,
        url: contactInfo.url,
        type: message.type,
      })

      return
    }
  }

  return sendPeerMessage
}
