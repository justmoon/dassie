import { createLogger } from "@dassie/lib-logger"

import { ledgerStore } from "../../accounting/stores/ledger"
import { IlpFulfillPacket, IlpType } from "../schemas/ilp-packet-codec"
import { requestIdMapSignal } from "../signals/request-id-map"
import {
  ResolvedIlpPacketEvent,
  resolvedIlpPacketTopic,
} from "../topics/resolved-ilp-packet"
import { createPacketSender } from "./send-packet"

const logger = createLogger("das:ilp-connector:process-result-packet")

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
      throw new Error(
        "Received response ILP packet which did not match any request ILP packet we sent"
      )
    }

    requestIdMap.read().delete(requestId)

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
