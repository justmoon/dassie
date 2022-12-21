import { createTopic } from "@dassie/lib-reactive"

import type { IlpPacket } from "../ilp-packet-codec"

interface IncomingIlpPacket {
  source: string
  packet: IlpPacket
  asUint8Array: Uint8Array
  requestId: number
}

export const incomingIlpPacketTopic = () => createTopic<IncomingIlpPacket>()
