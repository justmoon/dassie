import { createLogger } from "@dassie/lib-logger"
import { isFailure } from "@dassie/lib-type-utils"

import { applyPacketPrepareToLedger } from "../../accounting/functions/apply-interledger-packet"
import { Transfer, ledgerStore } from "../../accounting/stores/ledger"
import { createResolveIlpAddress } from "../../routing/functions/resolve-ilp-address"
import { createPacketSender } from "../functions/send-packet"
import { ProcessIncomingPacketParameters } from "../process-packet"
import { IlpErrorCode } from "../schemas/ilp-errors"
import { IlpPreparePacket } from "../schemas/ilp-packet-codec"
import { requestIdMapSignal } from "../signals/request-id-map"
import {
  PreparedIlpPacketEvent,
  preparedIlpPacketTopic,
} from "../topics/prepared-ilp-packet"
import { createTriggerRejection } from "./trigger-rejection"

const logger = createLogger("das:ilp-connector:process-prepare-packet")

export interface ProcessPreparePacketEnvironment {
  ledger: ReturnType<typeof ledgerStore>
  preparedIlpPacketTopicValue: ReturnType<typeof preparedIlpPacketTopic>
  requestIdMap: ReturnType<typeof requestIdMapSignal>
  resolveIlpAddress: ReturnType<typeof createResolveIlpAddress>
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
  resolveIlpAddress,
  sendPacket,
  triggerRejection,
}: ProcessPreparePacketEnvironment) => {
  return ({
    sourceEndpointInfo,
    parsedPacket,
    serializedPacket,
    requestId,
  }: ProcessPreparePacketParameters) => {
    logger.debug("received ILP prepare", {
      from: sourceEndpointInfo.ilpAddress,
      to: parsedPacket.destination,
      amount: parsedPacket.amount,
    })

    const outgoingRequestId = Math.trunc(Math.random() * 0xff_ff_ff_ff)

    const tentativeTransfers: Transfer[] = []

    const preparedIlpPacketEvent: PreparedIlpPacketEvent = {
      sourceEndpointInfo,
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
        sourceEndpointInfo.accountPath,
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
      source: sourceEndpointInfo.ilpAddress,
      destination: parsedPacket.destination,
      requestId: outgoingRequestId,
    })

    const destinationEndpointInfo = resolveIlpAddress(parsedPacket.destination)

    if (isFailure(destinationEndpointInfo)) {
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
        destinationEndpointInfo.accountPath,
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

    sendPacket(destinationEndpointInfo, preparedIlpPacketEvent)
    preparedIlpPacketTopicValue.emit(preparedIlpPacketEvent)
  }
}
