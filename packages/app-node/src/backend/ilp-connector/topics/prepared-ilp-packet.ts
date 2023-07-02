import { createTopic } from "@dassie/lib-reactive"

import { Transfer } from "../../accounting/stores/ledger"
import { EndpointInfo } from "../functions/send-packet"
import { IlpPreparePacket } from "../schemas/ilp-packet-codec"

export interface PreparedIlpPacketEvent {
  sourceEndpointInfo: EndpointInfo
  serializedPacket: Uint8Array
  parsedPacket: IlpPreparePacket
  incomingRequestId: number
  outgoingRequestId: number
  pendingTransfers: readonly Transfer[]
  timeoutAbort: AbortController
}

export const preparedIlpPacketTopic = () =>
  createTopic<PreparedIlpPacketEvent>()
