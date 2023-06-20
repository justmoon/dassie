import { createLogger } from "@dassie/lib-logger"

import { ledgerStore } from "../../accounting/stores/ledger"
import { routingTableSignal } from "../../routing/signals/routing-table"
import { IlpRejectPacket, IlpType } from "../schemas/ilp-packet-codec"
import { requestIdMapSignal } from "../signals/request-id-map"
import {
  ResolvedIlpPacketEvent,
  resolvedIlpPacketTopic,
} from "../topics/resolved-ilp-packet"
import { createPacketSender } from "./send-packet"

const logger = createLogger("das:ilp-connector:process-reject-packet")

export interface ProcessRejectPacketEnvironment {
  ledger: ReturnType<typeof ledgerStore>
  resolvedIlpPacketTopicValue: ReturnType<typeof resolvedIlpPacketTopic>
  requestIdMap: ReturnType<typeof requestIdMapSignal>
  routingTable: ReturnType<typeof routingTableSignal>
  sendPacket: ReturnType<typeof createPacketSender>
}

export interface ProcessRejectPacketParameters {
  serializedPacket: Uint8Array
  parsedPacket: { type: typeof IlpType.Reject } & IlpRejectPacket
  requestId: number
}

export const createProcessRejectPacket = ({
  ledger,
  resolvedIlpPacketTopicValue,
  requestIdMap,
  routingTable,
  sendPacket,
}: ProcessRejectPacketEnvironment) => {
  return ({
    parsedPacket,
    serializedPacket,
    requestId,
  }: ProcessRejectPacketParameters) => {
    logger.debug(`received ILP reject`, {
      requestId,
    })

    const prepare = requestIdMap.read().get(requestId)

    if (!prepare) {
      throw new Error(
        "Received response ILP packet which did not match any request ILP packet we sent"
      )
    }

    requestIdMap.read().delete(requestId)

    for (const transfer of prepare.pendingTransfers) {
      ledger.voidPendingTransfer(transfer)
    }

    const destinationInfo = routingTable.read().lookup(prepare.sourceIlpAddress)

    if (!destinationInfo) {
      throw new Error(
        `Failed to pass on packet result: No route found for origin ${prepare.sourceIlpAddress}`
      )
    }

    const resolvedIlpPacketEvent: ResolvedIlpPacketEvent = {
      prepare,
      parsedPacket,
      serializedPacket,
    }

    sendPacket(destinationInfo, resolvedIlpPacketEvent)
    resolvedIlpPacketTopicValue.emit(resolvedIlpPacketEvent)
  }
}
