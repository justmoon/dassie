import { createHash } from "node:crypto"

import { LedgerStore } from "../../accounting/stores/ledger"
import { DassieReactor } from "../../base/types/dassie-base"
import { connector as logger } from "../../logger/instances"
import { ProcessIncomingPacketParameters } from "../process-packet"
import { IlpType } from "../schemas/ilp-packet-codec"
import {
  ResolvedIlpPacketEvent,
  ResolvedIlpPacketTopic,
} from "../topics/resolved-ilp-packet"
import {
  PendingPacketsKey,
  PendingPacketsMap,
} from "../values/pending-packets-map"
import { SendPacket } from "./send-packet"

export const ProcessFulfillPacket = (reactor: DassieReactor) => {
  const ledgerStore = reactor.use(LedgerStore)
  const resolvedIlpPacketTopic = reactor.use(ResolvedIlpPacketTopic)
  const pendingPacketsMap = reactor.use(PendingPacketsMap)
  const sendPacket = reactor.use(SendPacket)

  return ({
    sourceEndpointInfo,
    parsedPacket,
    serializedPacket,
    requestId,
  }: ProcessIncomingPacketParameters<typeof IlpType.Fulfill>) => {
    logger.debug(`received ILP fulfill`, {
      requestId,
    })

    const pendingPacketKey: PendingPacketsKey = `${sourceEndpointInfo.ilpAddress}#${requestId}`
    const prepare = pendingPacketsMap.get(pendingPacketKey)

    if (!prepare) {
      logger.warn(
        "received fulfill packet which did not match any pending request",
        { requestId },
      )
      return
    }

    const hasher = createHash("sha256")
    hasher.update(parsedPacket.data.fulfillment)
    const fulfillmentHash = hasher.digest()

    if (!fulfillmentHash.equals(prepare.parsedPacket.executionCondition)) {
      logger.warn(
        "discarding ILP fulfill packet whose fulfillment hash does not match the corresponding execution condition",
        {
          fulfillment: parsedPacket.data.fulfillment,
          fulfillmentHash,
          executionCondition: prepare.parsedPacket.executionCondition,
        },
      )
      return
    }

    pendingPacketsMap.delete(pendingPacketKey)

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
