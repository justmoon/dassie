import { createLogger } from "@dassie/lib-logger"

import { ledgerStore } from "../../accounting/stores/ledger"
import { IlpRejectPacket, IlpType } from "../schemas/ilp-packet-codec"
import { requestIdMapSignal } from "../signals/request-id-map"
import {
  ResolvedIlpPacketEvent,
  resolvedIlpPacketTopic,
} from "../topics/resolved-ilp-packet"
import { createPacketSender } from "./send-packet"

const logger = createLogger("das:ilp-connector:process-reject-packet")

export interface ProcessRejectPacketEnvironment {
  ledger: ReturnType<typeof ledgerStore>
  resolvedIlpPacketTopicValue: ReturnType<typeof resolvedIlpPacketTopic>
  requestIdMap: ReturnType<typeof requestIdMapSignal>
  sendPacket: ReturnType<typeof createPacketSender>
}

export interface ProcessRejectPacketParameters {
  serializedPacket: Uint8Array
  parsedPacket: { type: typeof IlpType.Reject } & IlpRejectPacket
  requestId: number
}

export const createProcessRejectPacket = ({
  ledger,
  resolvedIlpPacketTopicValue,
  requestIdMap,
  sendPacket,
}: ProcessRejectPacketEnvironment) => {
  return ({
    parsedPacket,
    serializedPacket,
    requestId,
  }: ProcessRejectPacketParameters) => {
    logger.debug(`received ILP reject`, {
      requestId,
    })

    const prepare = requestIdMap.read().get(requestId)

    if (!prepare) {
      logger.warn(
        "received reject packet which did not match any pending request",
        { requestId }
      )
      return
    }

    requestIdMap.read().delete(requestId)

    prepare.timeoutAbort.abort()

    for (const transfer of prepare.pendingTransfers) {
      ledger.voidPendingTransfer(transfer)
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
