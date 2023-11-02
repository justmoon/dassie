import { Reactor } from "@dassie/lib-reactive"
import { UnreachableCaseError, isFailure } from "@dassie/lib-type-utils"

import { applyPacketPrepareToLedger } from "../../accounting/functions/apply-interledger-packet"
import { LedgerStore, Transfer } from "../../accounting/stores/ledger"
import { connector as logger } from "../../logger/instances"
import { ResolveIlpAddress } from "../../routing/functions/resolve-ilp-address"
import { MAX_PACKET_AMOUNT } from "../constants/max-packet-amount"
import { SendPacket } from "../functions/send-packet"
import { ProcessIncomingPacketParameters } from "../process-packet"
import { IlpErrorCode } from "../schemas/ilp-errors"
import { IlpPreparePacket } from "../schemas/ilp-packet-codec"
import { RequestIdMapSignal } from "../signals/request-id-map"
import {
  PreparedIlpPacketEvent,
  PreparedIlpPacketTopic,
} from "../topics/prepared-ilp-packet"
import { ScheduleTimeout } from "./schedule-timeout"
import { TriggerRejection } from "./trigger-rejection"

export type ProcessPreparePacketParameters = ProcessIncomingPacketParameters & {
  parsedPacket: IlpPreparePacket
}

export const ProcessPreparePacket = (reactor: Reactor) => {
  const ledgerStore = reactor.use(LedgerStore)
  const preparedIlpPacketTopic = reactor.use(PreparedIlpPacketTopic)
  const requestIdMapSignal = reactor.use(RequestIdMapSignal)
  const resolveIlpAddress = reactor.use(ResolveIlpAddress)
  const sendPacket = reactor.use(SendPacket)
  const triggerRejection = reactor.use(TriggerRejection)
  const scheduleTimeout = reactor.use(ScheduleTimeout)

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

    const pendingTransfers: Transfer[] = []

    const timeoutAbort = new AbortController()

    const preparedIlpPacketEvent: PreparedIlpPacketEvent = {
      sourceEndpointInfo,
      serializedPacket,
      parsedPacket,
      incomingRequestId: requestId,
      outgoingRequestId,
      pendingTransfers, // Note that we will still add transfers to this array
      timeoutAbort,
    }

    logger.debug("forwarding ILP prepare", {
      source: sourceEndpointInfo.ilpAddress,
      destination: parsedPacket.destination,
      requestId: outgoingRequestId,
    })

    requestIdMapSignal.read().set(outgoingRequestId, preparedIlpPacketEvent)

    scheduleTimeout({ requestId: outgoingRequestId, timeoutAbort })

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
      if (parsedPacket.amount > MAX_PACKET_AMOUNT) {
        triggerRejection({
          requestId: outgoingRequestId,
          errorCode: IlpErrorCode.F08_AMOUNT_TOO_LARGE,
          message: "Packet amount exceeds maximum allowed amount",
        })
        return
      }

      {
        const result = applyPacketPrepareToLedger(
          ledgerStore,
          sourceEndpointInfo.accountPath,
          parsedPacket,
          "incoming",
        )
        if (isFailure(result)) {
          switch (result.name) {
            case "InvalidAccountFailure": {
              triggerRejection({
                requestId: outgoingRequestId,
                errorCode: IlpErrorCode.T00_INTERNAL_ERROR,
                message: "Missing internal ledger account for inbound transfer",
              })
              return
            }
            case "ExceedsCreditsFailure": {
              triggerRejection({
                requestId: outgoingRequestId,
                errorCode: IlpErrorCode.T04_INSUFFICIENT_LIQUIDITY,
                message:
                  "Insufficient liquidity (connector account credit limit exceeded)",
              })
              return
            }
            case "ExceedsDebitsFailure": {
              triggerRejection({
                requestId: outgoingRequestId,
                errorCode: IlpErrorCode.T04_INSUFFICIENT_LIQUIDITY,
                message:
                  "Insufficient liquidity (source account debit limit exceeded)",
              })
              return
            }
            default: {
              throw new UnreachableCaseError(result)
            }
          }
        }
        pendingTransfers.push(result)
      }

      {
        const result = applyPacketPrepareToLedger(
          ledgerStore,
          destinationEndpointInfo.accountPath,
          parsedPacket,
          "outgoing",
        )
        if (isFailure(result)) {
          switch (result.name) {
            case "InvalidAccountFailure": {
              triggerRejection({
                requestId: outgoingRequestId,
                errorCode: IlpErrorCode.T00_INTERNAL_ERROR,
                message:
                  "Missing internal ledger account for outbound transfer",
              })
              return
            }
            case "ExceedsCreditsFailure": {
              triggerRejection({
                requestId: outgoingRequestId,
                errorCode: IlpErrorCode.T04_INSUFFICIENT_LIQUIDITY,
                message:
                  "Insufficient liquidity (destination account credit limit exceeded)",
              })
              return
            }
            case "ExceedsDebitsFailure": {
              triggerRejection({
                requestId: outgoingRequestId,
                errorCode: IlpErrorCode.T04_INSUFFICIENT_LIQUIDITY,
                message:
                  "Insufficient liquidity (connector account debit limit exceeded)",
              })
              return
            }
            default: {
              throw new UnreachableCaseError(result)
            }
          }
        }
        pendingTransfers.push(result)
      }
    }

    sendPacket(destinationEndpointInfo, preparedIlpPacketEvent)
    preparedIlpPacketTopic.emit(preparedIlpPacketEvent)
  }
}
