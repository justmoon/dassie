import { createTopic } from "@dassie/lib-reactive"

import type { IlpPacket } from "../ilp-packet-codec"

export interface OutgoingIlpPacket {
  destination: string
  packet: IlpPacket
  asUint8Array: Uint8Array
  requestId: number
}

export const outgoingIlpPacketBuffer = () => createTopic<OutgoingIlpPacket>()
