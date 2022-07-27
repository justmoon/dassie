import type { MarkOptional } from "ts-essentials"

import {
  Infer,
  ia5String,
  octetString,
  sequence,
  sequenceOf,
  uint8Number,
  uint64Bigint,
} from "@xen-ilp/lib-oer"

const HELLO_NEIGHBOR_PROOF_LENGTH = 32

export type XenMessage = XenHelloMessage

export enum XenMessageType {
  Hello = 0,
}

export interface XenHelloMessage
  extends Omit<Infer<typeof xenHelloMessage>, "signed"> {
  signed: XenHelloMessageSignedPortion
}

export type XenHelloMessageSignedPortion = Infer<
  typeof xenHelloMessageSignedPortion
>

const xenHelloMessage = sequence({
  method: uint8Number(),
  signed: octetString(),
  signature: octetString(64),
})

const xenHelloMessageSignedPortion = sequence({
  nodeId: ia5String(),
  sequence: uint64Bigint(),
  url: ia5String(),
  neighbors: sequenceOf(
    sequence({
      nodeId: ia5String(),
      proof: octetString(HELLO_NEIGHBOR_PROOF_LENGTH),
    })
  ),
})

export interface XenHelloNeighbor {
  nodeId: string
  proof: Uint8Array
}

export const parseMessage = (message: Uint8Array): XenMessage => {
  const method = message[0]

  switch (method) {
    case XenMessageType.Hello: {
      const helloMessageParseResult = xenHelloMessage.parse(message)
      if (!helloMessageParseResult.success) {
        throw new Error("Failed to parse hello message portion", {
          cause: helloMessageParseResult.failure,
        })
      }

      const { signed, signature } = helloMessageParseResult.value

      const signedPortionParseResult =
        xenHelloMessageSignedPortion.parse(signed)

      if (!signedPortionParseResult.success) {
        throw new Error("Failed to parse hello message signed portion", {
          cause: signedPortionParseResult.failure,
        })
      }

      const { nodeId, sequence, url, neighbors } =
        signedPortionParseResult.value

      return {
        method,
        signed: {
          nodeId,
          sequence,
          url,
          neighbors,
        },
        signature,
      }
    }
    default:
      throw new Error("Unknown Xen message type")
  }
}

export type XenMessageWithOptionalSignature = MarkOptional<
  XenMessage,
  "signature"
>

export function encodeMessage(message: XenMessage): Uint8Array
export function encodeMessage(
  message: XenMessageWithOptionalSignature,
  sign: (content: Uint8Array) => Uint8Array
): Uint8Array
export function encodeMessage(
  message: XenMessageWithOptionalSignature,
  sign?: (content: Uint8Array) => Uint8Array
): Uint8Array {
  switch (message.method) {
    case XenMessageType.Hello: {
      const signedMessageSerializeResult =
        xenHelloMessageSignedPortion.serialize(message.signed)

      if (!signedMessageSerializeResult.success) {
        throw new Error("Failed to serialize signed portion")
      }

      const messageSerializeResult = xenHelloMessage.serialize({
        method: 0,
        signed: signedMessageSerializeResult.value,
        signature:
          message.signature ?? sign!(signedMessageSerializeResult.value),
      })

      if (!messageSerializeResult.success) {
        throw new Error("Failed to serialize message")
      }

      return messageSerializeResult.value
    }
    default:
      throw new Error("Unknown Xen message type")
  }
}
