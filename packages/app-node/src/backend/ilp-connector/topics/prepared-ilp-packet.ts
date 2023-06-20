import { createTopic } from "@dassie/lib-reactive"

import { Transfer } from "../../accounting/stores/ledger"
import { IlpPreparePacket } from "../schemas/ilp-packet-codec"

export interface PreparedIlpPacketEvent {
  sourceIlpAddress: string
  ledgerAccountPath: string
  serializedPacket: Uint8Array
  parsedPacket: IlpPreparePacket
  incomingRequestId: number
  outgoingRequestId: number
  pendingTransfers: readonly Transfer[]
}

export const preparedIlpPacketTopic = () =>
  createTopic<PreparedIlpPacketEvent>()
