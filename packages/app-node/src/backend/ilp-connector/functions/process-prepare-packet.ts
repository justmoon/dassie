import { createLogger } from "@dassie/lib-logger"
import { isFailure } from "@dassie/lib-type-utils"

import { applyPacketPrepareToLedger } from "../../accounting/functions/apply-interledger-packet"
import { ledgerStore } from "../../accounting/stores/ledger"
import { createPacketSender } from "../functions/send-packet"
import { IlpPreparePacket } from "../ilp-packet-codec"
import { ProcessIncomingPacketParameters } from "../process-packet"
import { requestIdMapSignal } from "../signals/request-id-map"
import { routingTableSignal } from "../signals/routing-table"
import {
  PreparedIlpPacketEvent,
  preparedIlpPacketTopic,
} from "../topics/prepared-ilp-packet"
import { createGetLedgerPathForDestination } from "./get-ledger-path-for-destination"

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

    const preparedIlpPacketEvent: PreparedIlpPacketEvent = {
      sourceIlpAddress,
      ledgerAccountPath,
      serializedPacket,
      parsedPacket,
      incomingRequestId: requestId,
      outgoingRequestId,
    }

    if (parsedPacket.amount > 0n) {
      const result = applyPacketPrepareToLedger(
        ledger,
        ledgerAccountPath,
        parsedPacket,
        "incoming"
      )
      if (isFailure(result)) {
        // TODO: reject packet
        throw new Error(
          `failed to create transfer, invalid ${result.whichAccount} account ${result.accountPath}`
        )
      }
    }

    requestIdMap.read().set(outgoingRequestId, preparedIlpPacketEvent)

    logger.debug("forwarding ILP prepare", {
      source: sourceIlpAddress,
      destination: parsedPacket.destination,
      requestId: outgoingRequestId,
    })

    preparedIlpPacketTopicValue.emit(preparedIlpPacketEvent)

    const destinationInfo = routingTable.read().lookup(parsedPacket.destination)

    if (!destinationInfo) {
      throw new Error(
        `Failed to forward Interledger packet: No route found for destination ${parsedPacket.destination}`
      )
    }

    if (parsedPacket.amount > 0n) {
      applyPacketPrepareToLedger(
        ledger,
        getLedgerPathForDestination(destinationInfo),
        parsedPacket,
        "outgoing"
      )
    }

    sendPacket(destinationInfo, preparedIlpPacketEvent)
  }
}
