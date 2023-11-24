import { createHash } from "node:crypto"

import { Reactor } from "@dassie/lib-reactive"

import { LedgerStore } from "../../accounting/stores/ledger"
import { connector as logger } from "../../logger/instances"
import { IlpFulfillPacket, IlpType } from "../schemas/ilp-packet-codec"
import {
  ResolvedIlpPacketEvent,
  ResolvedIlpPacketTopic,
} from "../topics/resolved-ilp-packet"
import { RequestIdMap } from "../values/request-id-map"
import { SendPacket } from "./send-packet"

export interface ProcessFulfillPacketParameters {
  serializedPacket: Uint8Array
  parsedPacket: { type: typeof IlpType.Fulfill } & IlpFulfillPacket
  requestId: number
}

export const ProcessFulfillPacket = (reactor: Reactor) => {
  const ledgerStore = reactor.use(LedgerStore)
  const resolvedIlpPacketTopic = reactor.use(ResolvedIlpPacketTopic)
  const requestIdMap = reactor.use(RequestIdMap)
  const sendPacket = reactor.use(SendPacket)

  return ({
    parsedPacket,
    serializedPacket,
    requestId,
  }: ProcessFulfillPacketParameters) => {
    logger.debug(`received ILP fulfill`, {
      requestId,
    })

    const prepare = requestIdMap.get(requestId)

    if (!prepare) {
      logger.warn(
        "received fulfill packet which did not match any pending request",
        { requestId },
      )
      return
    }

    const hasher = createHash("sha256")
    hasher.update(parsedPacket.fulfillment)
    const fulfillmentHash = hasher.digest()

    if (!fulfillmentHash.equals(prepare.parsedPacket.executionCondition)) {
      logger.warn(
        "discarding ILP fulfill packet whose fulfillment hash does not match the corresponding execution condition",
        {
          fulfillment: parsedPacket.fulfillment,
          fulfillmentHash,
          executionCondition: prepare.parsedPacket.executionCondition,
        },
      )
      return
    }

    requestIdMap.delete(requestId)

    prepare.timeoutAbort.abort()

    for (const transfer of prepare.pendingTransfers) {
      ledgerStore.postPendingTransfer(transfer)
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
