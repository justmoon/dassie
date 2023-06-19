import { createLogger } from "@dassie/lib-logger"

import { applyPacketResultToLedger } from "../../accounting/functions/apply-interledger-packet"
import { ledgerStore } from "../../accounting/stores/ledger"
import { IlpPacket, IlpPreparePacket, IlpType } from "../ilp-packet-codec"
import { ProcessIncomingPacketParameters } from "../process-packet"
import { requestIdMapSignal } from "../signals/request-id-map"
import { routingTableSignal } from "../signals/routing-table"
import {
  ResolvedIlpPacketEvent,
  resolvedIlpPacketTopic,
} from "../topics/resolved-ilp-packet"
import { createGetLedgerPathForDestination } from "./get-ledger-path-for-destination"
import { createPacketSender } from "./send-packet"

const logger = createLogger("das:ilp-connector:process-result-packet")

export interface ProcessResultPacketEnvironment {
  ledger: ReturnType<typeof ledgerStore>
  resolvedIlpPacketTopicValue: ReturnType<typeof resolvedIlpPacketTopic>
  requestIdMap: ReturnType<typeof requestIdMapSignal>
  routingTable: ReturnType<typeof routingTableSignal>
  getLedgerPathForDestination: ReturnType<
    typeof createGetLedgerPathForDestination
  >
  sendPacket: ReturnType<typeof createPacketSender>
}

export type ProcessResultPacketParameters = ProcessIncomingPacketParameters & {
  parsedPacket: Exclude<IlpPacket, IlpPreparePacket>
}

export const createProcessResultPacket = ({
  ledger,
  resolvedIlpPacketTopicValue,
  requestIdMap,
  routingTable,
  getLedgerPathForDestination,
  sendPacket,
}: ProcessResultPacketEnvironment) => {
  return ({
    ledgerAccountPath,
    parsedPacket,
    serializedPacket,
    requestId,
  }: ProcessResultPacketParameters) => {
    const fulfillOrReject =
      parsedPacket.type === IlpType.Fulfill ? "fulfill" : "reject"
    logger.debug(`received ILP ${fulfillOrReject}`, {
      requestId,
    })

    const prepare = requestIdMap.read().get(requestId)

    if (!prepare) {
      throw new Error(
        "Received response ILP packet which did not match any request ILP packet we sent"
      )
    }

    requestIdMap.read().delete(requestId)

    const resolvedIlpPacketEvent: ResolvedIlpPacketEvent = {
      prepare,
      parsedPacket,
      serializedPacket,
    }

    if (prepare.parsedPacket.amount > 0n) {
      applyPacketResultToLedger(
        ledger,
        ledgerAccountPath,
        prepare.parsedPacket,
        fulfillOrReject
      )
    }

    resolvedIlpPacketTopicValue.emit(resolvedIlpPacketEvent)

    const destinationInfo = routingTable.read().lookup(prepare.sourceIlpAddress)

    if (!destinationInfo) {
      throw new Error(
        `Failed to pass on packet result: No route found for origin ${prepare.sourceIlpAddress}`
      )
    }

    if (prepare.parsedPacket.amount > 0n) {
      applyPacketResultToLedger(
        ledger,
        getLedgerPathForDestination(destinationInfo),
        prepare.parsedPacket,
        parsedPacket.type === IlpType.Fulfill ? "fulfill" : "reject"
      )
    }
    sendPacket(destinationInfo, resolvedIlpPacketEvent)
  }
}
