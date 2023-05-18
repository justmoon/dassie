import { createTopic } from "@dassie/lib-reactive"

import { IlpPreparePacket } from "../ilp-packet-codec"

export interface PreparedIlpPacketEvent {
  sourceIlpAddress: string
  ledgerAccountPath: string
  serializedPacket: Uint8Array
  parsedPacket: IlpPreparePacket
  incomingRequestId: number
  outgoingRequestId: number
}

export const preparedIlpPacketTopic = () =>
  createTopic<PreparedIlpPacketEvent>()
