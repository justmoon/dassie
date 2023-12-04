import { isFailure } from "@dassie/lib-type-utils"

import { Transfer } from "../../accounting/stores/ledger"
import { DassieReactor } from "../../base/types/dassie-base"
import { connector as logger } from "../../logger/instances"
import { PreparedPacketParameters, SendPacket } from "../functions/send-packet"
import {
  IlpPacket,
  IlpType,
  serializeIlpPacket,
} from "../schemas/ilp-packet-codec"
import {
  PreparedIlpPacketEvent,
  PreparedIlpPacketTopic,
} from "../topics/prepared-ilp-packet"
import { PendingPacketsMap } from "../values/pending-packets-map"
import { CalculatePreparePacketOutcome } from "./calculate-prepare-packet-outcome"
import { ProcessIncomingPacketParameters } from "./process-packet"
import { ScheduleTimeout } from "./schedule-timeout"
import { TriggerEarlyRejection } from "./trigger-early-rejection"

export const ProcessPreparePacket = (reactor: DassieReactor) => {
  const preparedIlpPacketTopic = reactor.use(PreparedIlpPacketTopic)
  const pendingPacketsMap = reactor.use(PendingPacketsMap)
  const sendPacket = reactor.use(SendPacket)
  const triggerEarlyRejection = reactor.use(TriggerEarlyRejection)
  const scheduleTimeout = reactor.use(ScheduleTimeout)
  const calculatePreparePacketOutcome = reactor.use(
    CalculatePreparePacketOutcome,
  )

  return ({
    sourceEndpointInfo,
    parsedPacket,
    serializedPacket,
    requestId,
  }: ProcessIncomingPacketParameters<typeof IlpType.Prepare>) => {
    logger.debug("received ILP prepare", {
      from: sourceEndpointInfo.ilpAddress,
      to: parsedPacket.data.destination,
      amount: parsedPacket.data.amount,
    })

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

    logger.debug("forwarding ILP prepare", {
      source: sourceEndpointInfo.ilpAddress,
      destination: parsedPacket.data.destination,
      requestId: outgoingRequestId,
    })

    const packetOutcome = calculatePreparePacketOutcome({
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

    pendingPacketsMap.set(
      `${destinationEndpointInfo.ilpAddress}#${outgoingRequestId}`,
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
}
