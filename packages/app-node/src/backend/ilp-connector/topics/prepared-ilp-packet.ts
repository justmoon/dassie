import { IlpPreparePacket } from "@dassie/lib-protocol-ilp"
import { createTopic } from "@dassie/lib-reactive"

import { Transfer } from "../../accounting/stores/ledger"
import { EndpointInfo } from "../functions/send-packet"

export interface PreparedIlpPacketEvent {
  readonly sourceEndpointInfo: EndpointInfo
  readonly serializedPacket: Uint8Array
  readonly parsedPacket: IlpPreparePacket
  readonly incomingRequestId: number | string
  readonly outgoingRequestId: number
  readonly pendingTransfers: readonly Transfer[]
  readonly timeoutAbort: AbortController
}

export const PreparedIlpPacketTopic = () =>
  createTopic<PreparedIlpPacketEvent>()
