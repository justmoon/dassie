import { Reader, Writer } from "oer-utils"
import type { MarkOptional } from "ts-essentials"

import {
  ia5String,
  octetString,
  sequence,
  sequenceOf,
  uint64Bigint,
} from "@xen-ilp/lib-oer"

const HELLO_NEIGHBOR_PROOF_LENGTH = 32

export type XenMessage = XenHelloMessage

export enum XenMessageType {
  Hello = 0,
}

export interface XenHelloMessage {
  method: XenMessageType.Hello
  signed: XenHelloMessageSignedPortion
  signature: Buffer
}

const xenHelloMessageSignedPortion = sequence({
  nodeId: ia5String(),
  sequence: uint64Bigint(),
  url: ia5String(),
  neighbors: sequenceOf(
    sequence({
      nodeId: ia5String(),
      proof: octetString({ fixedLength: HELLO_NEIGHBOR_PROOF_LENGTH }),
    })
  ),
})
export interface XenHelloMessageSignedPortion {
  nodeId: string
  sequence: bigint
  url: string
  neighbors: XenHelloNeighbor[]
}

export interface XenHelloNeighbor {
  nodeId: string
  proof: Uint8Array
}

export const parseMessage = (message: Uint8Array): XenMessage => {
  const reader = Reader.from(Buffer.from(message))
  const method = reader.readUInt8Number()

  switch (method) {
    case XenMessageType.Hello: {
      const signed = reader.readVarOctetString()
      const signature = reader.readOctetString(64)

      const signedPortionParseResult =
        xenHelloMessageSignedPortion.parse(signed)

      if (!signedPortionParseResult.success) {
        throw new Error("Failed to parse signed portion")
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

const writeSignature = (
  writer: Writer,
  message: XenMessageWithOptionalSignature,
  signed: Buffer,
  sign?: (content: Buffer) => Buffer
) => {
  if (message.signature != null) {
    writer.writeOctetString(message.signature, 64)
  } else if (sign != null) {
    writer.writeOctetString(sign(signed), 64)
  } else {
    throw new TypeError(
      "encodeMessage requires that a signing function is provided as the second parameter if message.signature is null"
    )
  }
}

export function encodeMessage(message: XenMessage): Buffer
export function encodeMessage(
  message: XenMessageWithOptionalSignature,
  sign: (content: Buffer) => Buffer
): Buffer
export function encodeMessage(
  message: XenMessageWithOptionalSignature,
  sign?: (content: Buffer) => Buffer
): Buffer {
  const writer = new Writer()
  writer.writeUInt8(message.method)

  switch (message.method) {
    case XenMessageType.Hello: {
      const signed = xenHelloMessageSignedPortion.serialize(message.signed)

      if (!signed.success) {
        throw new Error("Failed to serialize signed portion")
      }

      writer.writeVarOctetString(Buffer.from(signed.value))
      writeSignature(writer, message, Buffer.from(signed.value), sign)

      return writer.getBuffer()
    }
  }
}
