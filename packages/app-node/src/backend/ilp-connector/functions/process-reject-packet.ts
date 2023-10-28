import { LedgerStore } from "../../accounting/stores/ledger"
import { connector as logger } from "../../logger/instances"
import { IlpRejectPacket, IlpType } from "../schemas/ilp-packet-codec"
import { RequestIdMapSignal } from "../signals/request-id-map"
import {
  ResolvedIlpPacketEvent,
  ResolvedIlpPacketTopic,
} from "../topics/resolved-ilp-packet"
import { createPacketSender } from "./send-packet"

export interface ProcessRejectPacketEnvironment {
  ledger: ReturnType<typeof LedgerStore>
  resolvedIlpPacketTopic: ReturnType<typeof ResolvedIlpPacketTopic>
  requestIdMap: ReturnType<typeof RequestIdMapSignal>
  sendPacket: ReturnType<typeof createPacketSender>
}

export interface ProcessRejectPacketParameters {
  serializedPacket: Uint8Array
  parsedPacket: { type: typeof IlpType.Reject } & IlpRejectPacket
  requestId: number
}

export const createProcessRejectPacket = ({
  ledger,
  resolvedIlpPacketTopic,
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
        { requestId },
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
    resolvedIlpPacketTopic.emit(resolvedIlpPacketEvent)
  }
}
