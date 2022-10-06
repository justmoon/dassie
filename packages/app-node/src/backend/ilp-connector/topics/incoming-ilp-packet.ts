import { createTopic } from "@dassie/lib-reactive"

interface IncomingIlpPacket {
  source: string
  packet: Uint8Array
  requestId: number
}

export const incomingIlpPacketBuffer = () => createTopic<IncomingIlpPacket>()
