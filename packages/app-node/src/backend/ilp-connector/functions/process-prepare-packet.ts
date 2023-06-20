import { createLogger } from "@dassie/lib-logger"
import { isFailure } from "@dassie/lib-type-utils"

import { applyPacketPrepareToLedger } from "../../accounting/functions/apply-interledger-packet"
import { Transfer, ledgerStore } from "../../accounting/stores/ledger"
import { createPacketSender } from "../functions/send-packet"
import { ProcessIncomingPacketParameters } from "../process-packet"
import { IlpErrorCode } from "../schemas/ilp-errors"
import { IlpPreparePacket } from "../schemas/ilp-packet-codec"
import { requestIdMapSignal } from "../signals/request-id-map"
import { routingTableSignal } from "../signals/routing-table"
import {
  PreparedIlpPacketEvent,
  preparedIlpPacketTopic,
} from "../topics/prepared-ilp-packet"
import { createGetLedgerPathForDestination } from "./get-ledger-path-for-destination"
import { createTriggerRejection } from "./trigger-rejection"

const logger = createLogger("das:ilp-connector:process-prepare-packet")

export interface ProcessPreparePacketEnvironment {
  ledger: ReturnType<typeof ledgerStore>
  preparedIlpPacketTopicValue: ReturnType<typeof preparedIlpPacketTopic>
  requestIdMap: ReturnType<typeof requestIdMapSignal>
  routingTable: ReturnType<typeof routingTableSignal>
  getLedgerPathForDestination: ReturnType<
    typeof createGetLedgerPathForDestination
  >
  sendPacket: ReturnType<typeof createPacketSender>
  triggerRejection: ReturnType<typeof createTriggerRejection>
}

export type ProcessPreparePacketParameters = ProcessIncomingPacketParameters & {
  parsedPacket: IlpPreparePacket
}

export const createProcessPreparePacket = ({
  ledger,
  preparedIlpPacketTopicValue,
  requestIdMap,
  routingTable,
  getLedgerPathForDestination,
  sendPacket,
  triggerRejection,
}: ProcessPreparePacketEnvironment) => {
  return ({
    sourceIlpAddress,
    parsedPacket,
    ledgerAccountPath,
    serializedPacket,
    requestId,
  }: ProcessPreparePacketParameters) => {
    logger.debug("received ILP prepare", {
      from: sourceIlpAddress,
      to: parsedPacket.destination,
      amount: parsedPacket.amount,
    })

    const outgoingRequestId = Math.trunc(Math.random() * 0xff_ff_ff_ff)

    const tentativeTransfers: Transfer[] = []

    const preparedIlpPacketEvent: PreparedIlpPacketEvent = {
      sourceIlpAddress,
      ledgerAccountPath,
      serializedPacket,
      parsedPacket,
      incomingRequestId: requestId,
      outgoingRequestId,
      pendingTransfers: tentativeTransfers,
    }

    requestIdMap.read().set(outgoingRequestId, preparedIlpPacketEvent)

    if (parsedPacket.amount > 0n) {
      const result = applyPacketPrepareToLedger(
        ledger,
        ledgerAccountPath,
        parsedPacket,
        "incoming"
      )
      if (isFailure(result)) {
        triggerRejection({
          requestId: outgoingRequestId,
          errorCode: IlpErrorCode.T00_INTERNAL_ERROR,
          message: "Missing internal ledger account for inbound transfer",
        })
        return
      }
      tentativeTransfers.push(result)
    }

    logger.debug("forwarding ILP prepare", {
      source: sourceIlpAddress,
      destination: parsedPacket.destination,
      requestId: outgoingRequestId,
    })

    const destinationInfo = routingTable.read().lookup(parsedPacket.destination)

    if (!destinationInfo) {
      triggerRejection({
        requestId: outgoingRequestId,
        errorCode: IlpErrorCode.F02_UNREACHABLE,
        message: "No route found for destination",
      })
      return
    }

    if (parsedPacket.amount > 0n) {
      const result = applyPacketPrepareToLedger(
        ledger,
        getLedgerPathForDestination(destinationInfo),
        parsedPacket,
        "outgoing"
      )
      if (isFailure(result)) {
        triggerRejection({
          requestId: outgoingRequestId,
          errorCode: IlpErrorCode.T00_INTERNAL_ERROR,
          message: "Missing internal ledger account for outbound transfer",
        })
        return
      }
    }

    sendPacket(destinationInfo, preparedIlpPacketEvent)
    preparedIlpPacketTopicValue.emit(preparedIlpPacketEvent)
  }
}
