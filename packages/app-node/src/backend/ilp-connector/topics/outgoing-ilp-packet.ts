import { createTopic } from "@dassie/lib-reactive"

import type { IlpPacketWithAttachedPrepare } from "./incoming-ilp-packet"

export interface OutgoingIlpPacket {
  destination: string
  source: string
  packet: IlpPacketWithAttachedPrepare
  asUint8Array: Uint8Array
  requestId: number
}

export const outgoingIlpPacketBuffer = () => createTopic<OutgoingIlpPacket>()
