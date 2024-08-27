import { type Packet, streamPacketSchema } from "./schema"

export function deserializePacket(data: Uint8Array): Packet {
  const result = streamPacketSchema.parseOrThrow(data)

  return result.value
}

export function serializePacket(packet: Packet): Uint8Array {
  const serializationResult = streamPacketSchema.serializeOrThrow(packet)

  return serializationResult
}
