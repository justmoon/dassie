import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"
import { UnreachableCaseError } from "@dassie/lib-type-utils"

import { ledgerStore } from "../accounting/stores/ledger"
import { nodeIlpAddressSignal } from "./computed/node-ilp-address"
import { createGetLedgerPathForDestination } from "./functions/get-ledger-path-for-destination"
import { createProcessFulfillPacket } from "./functions/process-fulfill-packet"
import { createProcessPreparePacket } from "./functions/process-prepare-packet"
import { createProcessRejectPacket } from "./functions/process-reject-packet"
import { createPacketSender } from "./functions/send-packet"
import { createTriggerRejection } from "./functions/trigger-rejection"
import { IlpPacket, IlpType, parseIlpPacket } from "./schemas/ilp-packet-codec"
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
    const ownIlpAddress = sig.use(nodeIlpAddressSignal)

    const getLedgerPathForDestination = createGetLedgerPathForDestination(sig)
    const sendPacket = createPacketSender(sig)

    const processFulfillPacket = createProcessFulfillPacket({
      ledger,
      resolvedIlpPacketTopicValue,
      requestIdMap,
      routingTable,
      sendPacket,
    })
    const processRejectPacket = createProcessRejectPacket({
      ledger,
      resolvedIlpPacketTopicValue,
      requestIdMap,
      routingTable,
      sendPacket,
    })
    const triggerRejection = createTriggerRejection({
      ownIlpAddress,
      processRejectPacket,
    })
    const processPreparePacket = createProcessPreparePacket({
      ledger,
      preparedIlpPacketTopicValue,
      requestIdMap,
      routingTable,
      getLedgerPathForDestination,
      sendPacket,
      triggerRejection,
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

        switch (parsedPacket.type) {
          case IlpType.Prepare: {
            processPreparePacket({
              sourceIlpAddress,
              ledgerAccountPath,
              serializedPacket,
              parsedPacket,
              requestId,
            })
            break
          }
          case IlpType.Fulfill: {
            processFulfillPacket({
              serializedPacket,
              parsedPacket,
              requestId,
            })
            break
          }
          case IlpType.Reject: {
            processRejectPacket({
              serializedPacket,
              parsedPacket,
              requestId,
            })
            break
          }
          default: {
            throw new UnreachableCaseError(parsedPacket)
          }
        }
      },
    }
  })
