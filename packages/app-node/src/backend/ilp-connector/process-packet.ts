import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { ledgerStore } from "../accounting/stores/ledger"
import { createGetLedgerPathForDestination } from "./functions/get-ledger-path-for-destination"
import { createProcessPreparePacket } from "./functions/process-prepare-packet"
import { createProcessResultPacket } from "./functions/process-result-packet"
import { createPacketSender } from "./functions/send-packet"
import { IlpPacket, IlpType, parseIlpPacket } from "./ilp-packet-codec"
import { requestIdMapSignal } from "./signals/request-id-map"
import { routingTableSignal } from "./signals/routing-table"
import { preparedIlpPacketTopic } from "./topics/prepared-ilp-packet"
import { resolvedIlpPacketTopic } from "./topics/resolved-ilp-packet"

const logger = createLogger("das:node:ilp-connector:process-packet")

export interface ProcessIncomingPacketParameters {
  sourceIlpAddress: string
  ledgerAccountPath: string
  serializedPacket: Uint8Array
  parsedPacket?: IlpPacket | undefined
  requestId: number
}

export const processPacket = () =>
  createActor((sig) => {
    const ledger = sig.use(ledgerStore)
    const preparedIlpPacketTopicValue = sig.use(preparedIlpPacketTopic)
    const resolvedIlpPacketTopicValue = sig.use(resolvedIlpPacketTopic)
    const requestIdMap = sig.use(requestIdMapSignal)
    const routingTable = sig.use(routingTableSignal)

    const getLedgerPathForDestination = createGetLedgerPathForDestination(sig)
    const sendPacket = createPacketSender(sig)

    const processPreparePacket = createProcessPreparePacket({
      ledger,
      preparedIlpPacketTopicValue,
      requestIdMap,
      routingTable,
      getLedgerPathForDestination,
      sendPacket,
    })
    const processResultPacket = createProcessResultPacket({
      ledger,
      resolvedIlpPacketTopicValue,
      requestIdMap,
      routingTable,
      getLedgerPathForDestination,
      sendPacket,
    })

    return {
      handle: ({
        sourceIlpAddress,
        ledgerAccountPath,
        serializedPacket,
        requestId,
        parsedPacket: optionalParsedPacket,
      }: ProcessIncomingPacketParameters) => {
        logger.debug("handle interledger packet", {
          from: sourceIlpAddress,
        })

        // Parse packet if not already done
        const parsedPacket =
          optionalParsedPacket ?? parseIlpPacket(serializedPacket)

        if (parsedPacket.type === IlpType.Prepare) {
          processPreparePacket({
            sourceIlpAddress,
            ledgerAccountPath,
            serializedPacket,
            parsedPacket,
            requestId,
          })
        } else {
          processResultPacket({
            sourceIlpAddress,
            ledgerAccountPath,
            serializedPacket,
            parsedPacket,
            requestId,
          })
        }
      },
    }
  })
