import { Reactor } from "@dassie/lib-reactive"

import { LedgerStore } from "../../accounting/stores/ledger"
import { connector as logger } from "../../logger/instances"
import { IlpRejectPacket, IlpType } from "../schemas/ilp-packet-codec"
import { RequestIdMapSignal } from "../signals/request-id-map"
import {
  ResolvedIlpPacketEvent,
  ResolvedIlpPacketTopic,
} from "../topics/resolved-ilp-packet"
import { SendPacket } from "./send-packet"

export interface ProcessRejectPacketParameters {
  serializedPacket: Uint8Array
  parsedPacket: { type: typeof IlpType.Reject } & IlpRejectPacket
  requestId: number
}

export const ProcessRejectPacket = (reactor: Reactor) => {
  const ledgerStore = reactor.use(LedgerStore)
  const resolvedIlpPacketTopic = reactor.use(ResolvedIlpPacketTopic)
  const requestIdMapSignal = reactor.use(RequestIdMapSignal)
  const sendPacket = reactor.use(SendPacket)

  return ({
    parsedPacket,
    serializedPacket,
    requestId,
  }: ProcessRejectPacketParameters) => {
    logger.debug(`received ILP reject`, {
      requestId,
    })

    const prepare = requestIdMapSignal.read().get(requestId)

    if (!prepare) {
      logger.warn(
        "received reject packet which did not match any pending request",
        { requestId },
      )
      return
    }

    requestIdMapSignal.read().delete(requestId)

    prepare.timeoutAbort.abort()

    for (const transfer of prepare.pendingTransfers) {
      ledgerStore.voidPendingTransfer(transfer)
    }

    const resolvedIlpPacketEvent: ResolvedIlpPacketEvent = {
      prepare,
      parsedPacket,
      serializedPacket,
      destinationEndpointInfo: prepare.sourceEndpointInfo,
    }

    sendPacket(resolvedIlpPacketEvent)
    resolvedIlpPacketTopic.emit(resolvedIlpPacketEvent)
  }
}
