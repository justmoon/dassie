import { createHash } from "node:crypto"

import { ledgerStore } from "../../accounting/stores/ledger"
import { connector as logger } from "../../logger/instances"
import { IlpFulfillPacket, IlpType } from "../schemas/ilp-packet-codec"
import { requestIdMapSignal } from "../signals/request-id-map"
import {
  ResolvedIlpPacketEvent,
  resolvedIlpPacketTopic,
} from "../topics/resolved-ilp-packet"
import { createPacketSender } from "./send-packet"

export interface ProcessFulfillPacketEnvironment {
  ledger: ReturnType<typeof ledgerStore>
  resolvedIlpPacketTopicValue: ReturnType<typeof resolvedIlpPacketTopic>
  requestIdMap: ReturnType<typeof requestIdMapSignal>
  sendPacket: ReturnType<typeof createPacketSender>
}

export interface ProcessFulfillPacketParameters {
  serializedPacket: Uint8Array
  parsedPacket: { type: typeof IlpType.Fulfill } & IlpFulfillPacket
  requestId: number
}

export const createProcessFulfillPacket = ({
  ledger,
  resolvedIlpPacketTopicValue,
  requestIdMap,
  sendPacket,
}: ProcessFulfillPacketEnvironment) => {
  return ({
    parsedPacket,
    serializedPacket,
    requestId,
  }: ProcessFulfillPacketParameters) => {
    logger.debug(`received ILP fulfill`, {
      requestId,
    })

    const prepare = requestIdMap.read().get(requestId)

    if (!prepare) {
      logger.warn(
        "received fulfill packet which did not match any pending request",
        { requestId }
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
        }
      )
      return
    }

    requestIdMap.read().delete(requestId)

    prepare.timeoutAbort.abort()

    for (const transfer of prepare.pendingTransfers) {
      ledger.postPendingTransfer(transfer)
    }

    const resolvedIlpPacketEvent: ResolvedIlpPacketEvent = {
      prepare,
      parsedPacket,
      serializedPacket,
    }

    sendPacket(prepare.sourceEndpointInfo, resolvedIlpPacketEvent)
    resolvedIlpPacketTopicValue.emit(resolvedIlpPacketEvent)
  }
}
