import { IlpType } from "@dassie/lib-protocol-ilp"

import { LedgerStore } from "../../accounting/stores/ledger"
import type { DassieReactor } from "../../base/types/dassie-base"
import { connector as logger } from "../../logger/instances"
import {
  type ResolvedIlpPacketEvent,
  ResolvedIlpPacketTopic,
} from "../topics/resolved-ilp-packet"
import { getUniqueEndpointId } from "../utils/get-unique-endpoint-id"
import {
  type PendingPacketsKey,
  PendingPacketsMap,
} from "../values/pending-packets-map"
import type { ProcessIncomingPacketParameters } from "./process-packet"
import { SendPacket } from "./send-packet"

export const ProcessRejectPacket = (reactor: DassieReactor) => {
  const ledgerStore = reactor.use(LedgerStore)
  const resolvedIlpPacketTopic = reactor.use(ResolvedIlpPacketTopic)
  const pendingPacketsMap = reactor.use(PendingPacketsMap)
  const sendPacket = reactor.use(SendPacket)

  function processRejectPacket({
    sourceEndpointInfo,
    parsedPacket,
    serializedPacket,
    requestId,
  }: ProcessIncomingPacketParameters<typeof IlpType.Reject>) {
    logger.debug?.(`received ILP reject`, {
      requestId,
      code: parsedPacket.data.code,
      message: parsedPacket.data.message,
    })

    const pendingPacketKey: PendingPacketsKey = `${getUniqueEndpointId(sourceEndpointInfo)}#${requestId}`
    const prepare = pendingPacketsMap.get(pendingPacketKey)

    if (!prepare) {
      logger.warn(
        "received reject packet which did not match any pending request",
        { requestId },
      )
      return
    }

    pendingPacketsMap.delete(pendingPacketKey)

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

  return processRejectPacket
}
