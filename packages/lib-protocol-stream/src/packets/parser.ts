import { type StreamPacket, streamPacketSchema } from "./schema"

export function deserializePacket(data: Uint8Array): StreamPacket {
  const result = streamPacketSchema.parseOrThrow(data)

  return result.value
}

export function serializePacket(packet: StreamPacket): Uint8Array {
  const serializationResult = streamPacketSchema.serializeOrThrow(packet)

  return serializationResult
}
