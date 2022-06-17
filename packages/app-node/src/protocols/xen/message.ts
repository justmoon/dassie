import { Reader, Writer } from "oer-utils"

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

export interface XenHelloMessageSignedPortion {
  nodeId: string
  sequence: number
  neighbors: Array<XenHelloNeighbor>
}

export interface XenHelloNeighbor {
  nodeId: string
  proof: Buffer
}

export interface XenUnknownMessage {
  method: number
}

export const parseMessage = (
  message: Buffer
): XenMessage | XenUnknownMessage => {
  const reader = Reader.from(message)
  const method = reader.readUInt8Number()

  switch (method) {
    case XenMessageType.Hello: {
      const signed = reader.readVarOctetString()
      const signature = reader.readOctetString(64)
      const signedReader = Reader.from(signed)
      const nodeId = signedReader.readVarOctetString().toString("ascii")
      const sequence = signedReader.readUInt32Number()
      const neighborCount = signedReader.readUInt16Number()
      const neighbors = new Array(neighborCount)

      for (let i = 0; i < neighborCount; i++) {
        const neighborDefinition = signedReader.readVarOctetString()
        const neighborReader = Reader.from(neighborDefinition)
        const nodeId = neighborReader.readVarOctetString().toString("ascii")
        const proof = neighborReader.readOctetString(
          HELLO_NEIGHBOR_PROOF_LENGTH
        )

        neighbors[i] = {
          nodeId,
          proof,
        }
      }

      return {
        method,
        signed: {
          nodeId,
          sequence,
          neighbors,
        },
        signature,
      }
    }
    default:
      return { method }
  }
}

export const encodeMessage = (
  message: XenMessage,
  sign: (content: Buffer) => Buffer
): Buffer => {
  const writer = new Writer()
  writer.writeUInt8(message.method)

  switch (message.method) {
    case XenMessageType.Hello: {
      const signedWriter = new Writer()

      signedWriter.writeVarOctetString(
        Buffer.from(message.signed.nodeId, "ascii")
      )
      signedWriter.writeUInt32(message.signed.sequence)
      signedWriter.writeUInt16(message.signed.neighbors.length)

      for (const neighbor of message.signed.neighbors) {
        const neighborWriter = new Writer()
        neighborWriter.writeVarOctetString(
          Buffer.from(neighbor.nodeId, "ascii")
        )
        neighborWriter.writeOctetString(
          neighbor.proof,
          HELLO_NEIGHBOR_PROOF_LENGTH
        )
        signedWriter.writeVarOctetString(neighborWriter.getBuffer())
      }

      const signed = signedWriter.getBuffer()

      writer.writeVarOctetString(signed)
      writer.writeOctetString(sign(signed), 64)

      return writer.getBuffer()
    }
  }
}
