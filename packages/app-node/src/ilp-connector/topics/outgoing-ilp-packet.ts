import { createTopic } from "@dassie/lib-reactive"

interface OutgoingIlpPacket {
  destination: string
  packet: Uint8Array
  amount: bigint
  requestId: number
  isResponse: boolean
}

export const outgoingIlpPacketBuffer = () => createTopic<OutgoingIlpPacket>()
