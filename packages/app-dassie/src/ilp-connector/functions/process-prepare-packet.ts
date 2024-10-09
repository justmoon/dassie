import {
  type IlpPacket,
  IlpType,
  serializeIlpPacket,
} from "@dassie/lib-protocol-ilp"
import { isFailure } from "@dassie/lib-type-utils"

import type { Transfer } from "../../accounting/stores/ledger"
import type { DassieReactor } from "../../base/types/dassie-base"
import { connector as logger } from "../../logger/instances"
import {
  type PreparedPacketParameters,
  SendPacket,
} from "../functions/send-packet"
import {
  type PreparedIlpPacketEvent,
  PreparedIlpPacketTopic,
} from "../topics/prepared-ilp-packet"
import { getUniqueEndpointId } from "../utils/get-unique-endpoint-id"
import { PendingPacketsMap } from "../values/pending-packets-map"
import {
  CalculatePreparePacketOutcome,
  type PreparePacketOutcome,
} from "./calculate-prepare-packet-outcome"
import { GetEndpointIlpAddress } from "./get-endpoint-ilp-address"
import type { ProcessIncomingPacketParameters } from "./process-packet"
import { ScheduleTimeout } from "./schedule-timeout"
import { TriggerEarlyRejection } from "./trigger-early-rejection"

interface AdditionalPreparePacketParameters {
  predeterminedOutcome?: PreparePacketOutcome | undefined
}

export const ProcessPreparePacket = (reactor: DassieReactor) => {
  const preparedIlpPacketTopic = reactor.use(PreparedIlpPacketTopic)
  const pendingPacketsMap = reactor.use(PendingPacketsMap)
  const sendPacket = reactor.use(SendPacket)
  const triggerEarlyRejection = reactor.use(TriggerEarlyRejection)
  const scheduleTimeout = reactor.use(ScheduleTimeout)
  const calculatePreparePacketOutcome = reactor.use(
    CalculatePreparePacketOutcome,
  )
  const getEndpointIlpAddress = reactor.use(GetEndpointIlpAddress)

  function processPreparePacket({
    sourceEndpointInfo,
    parsedPacket,
    serializedPacket,
    requestId,
    predeterminedOutcome,
  }: ProcessIncomingPacketParameters<typeof IlpType.Prepare> &
    AdditionalPreparePacketParameters) {
    const outgoingRequestId = Math.trunc(Math.random() * 0xff_ff_ff_ff)

    const pendingTransfers: Transfer[] = []

    const timeoutAbort = new AbortController()

    const preparedIlpPacketEvent: PreparedIlpPacketEvent = {
      sourceEndpointInfo,
      serializedPacket,
      parsedPacket: parsedPacket.data,
      incomingRequestId: requestId,
      outgoingRequestId,
      pendingTransfers, // Note that we will still add transfers to this array
      timeoutAbort,
    }

    logger.debug?.("processing ILP prepare", {
      from: getEndpointIlpAddress(sourceEndpointInfo),
      to: parsedPacket.data.destination,
      amount: parsedPacket.data.amount,
      requestId: outgoingRequestId,
    })

    const packetOutcome =
      predeterminedOutcome ??
      calculatePreparePacketOutcome({
        sourceEndpointInfo,
        parsedPacket,
      })

    if (isFailure(packetOutcome)) {
      triggerEarlyRejection({
        prepare: preparedIlpPacketEvent,
        failure: packetOutcome,
      })
      return
    }

    const {
      destinationEndpointInfo,
      outgoingAmount,
      outgoingExpiry,
      transfers,
    } = packetOutcome

    logger.debug?.("forwarding ILP prepare", {
      from: getEndpointIlpAddress(sourceEndpointInfo),
      to: parsedPacket.data.destination,
      amount: parsedPacket.data.amount,
      requestId: outgoingRequestId,
      nextHop: getEndpointIlpAddress(destinationEndpointInfo),
    })

    pendingPacketsMap.set(
      `${getUniqueEndpointId(destinationEndpointInfo)}#${outgoingRequestId}`,
      preparedIlpPacketEvent,
    )

    scheduleTimeout({
      sourceEndpointInfo: destinationEndpointInfo,
      requestId: outgoingRequestId,
      timeoutAbort,
    })

    pendingTransfers.push(...transfers)

    const outgoingPacket: IlpPacket = {
      type: IlpType.Prepare,
      data: {
        amount: outgoingAmount,
        expiresAt: outgoingExpiry,
        executionCondition: parsedPacket.data.executionCondition,
        destination: parsedPacket.data.destination,
        data: parsedPacket.data.data,
      },
    }

    const outgoingPacketEvent: PreparedPacketParameters = {
      parsedPacket: outgoingPacket,
      serializedPacket: serializeIlpPacket(outgoingPacket),
      outgoingRequestId,
      sourceEndpointInfo,
      destinationEndpointInfo,
    }

    sendPacket(outgoingPacketEvent)
    preparedIlpPacketTopic.emit(preparedIlpPacketEvent)
  }

  return processPreparePacket
}
