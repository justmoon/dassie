import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"
import { UnreachableCaseError } from "@dassie/lib-type-utils"

import { ledgerStore } from "../accounting/stores/ledger"
import { ilpAllocationSchemeSignal } from "../config/computed/ilp-allocation-scheme"
import { nodeTableStore } from "../peer-protocol/stores/node-table"
import { createResolveIlpAddress } from "../routing/functions/resolve-ilp-address"
import { routingTableSignal } from "../routing/signals/routing-table"
import { nodeIlpAddressSignal } from "./computed/node-ilp-address"
import { createProcessFulfillPacket } from "./functions/process-fulfill-packet"
import { createProcessPreparePacket } from "./functions/process-prepare-packet"
import { createProcessRejectPacket } from "./functions/process-reject-packet"
import { createScheduleTimeout } from "./functions/schedule-timeout"
import { EndpointInfo, createPacketSender } from "./functions/send-packet"
import { createTriggerRejection } from "./functions/trigger-rejection"
import { IlpPacket, IlpType, parseIlpPacket } from "./schemas/ilp-packet-codec"
import { requestIdMapSignal } from "./signals/request-id-map"
import { preparedIlpPacketTopic } from "./topics/prepared-ilp-packet"
import { resolvedIlpPacketTopic } from "./topics/resolved-ilp-packet"

const logger = createLogger("das:node:ilp-connector:process-packet")

export interface ProcessIncomingPacketParameters {
  sourceEndpointInfo: EndpointInfo
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
    const nodeTable = sig.use(nodeTableStore)
    const ilpAllocationScheme = sig.use(ilpAllocationSchemeSignal)
    const ownIlpAddress = sig.use(nodeIlpAddressSignal)

    const resolveIlpAddress = createResolveIlpAddress({
      routingTable,
      nodeTable,
      ilpAllocationScheme,
    })
    const sendPacket = createPacketSender(sig)

    const processFulfillPacket = createProcessFulfillPacket({
      ledger,
      resolvedIlpPacketTopicValue,
      requestIdMap,
      sendPacket,
    })
    const processRejectPacket = createProcessRejectPacket({
      ledger,
      resolvedIlpPacketTopicValue,
      requestIdMap,
      sendPacket,
    })
    const triggerRejection = createTriggerRejection({
      ownIlpAddress,
      processRejectPacket,
    })
    const scheduleTimeout = createScheduleTimeout({
      triggerRejection,
    })
    const processPreparePacket = createProcessPreparePacket({
      ledger,
      preparedIlpPacketTopicValue,
      requestIdMap,
      resolveIlpAddress,
      sendPacket,
      triggerRejection,
      scheduleTimeout,
    })

    return {
      handle: ({
        sourceEndpointInfo,
        serializedPacket,
        requestId,
        parsedPacket: optionalParsedPacket,
      }: ProcessIncomingPacketParameters) => {
        logger.debug("handle interledger packet", {
          from: sourceEndpointInfo.ilpAddress,
        })

        // Parse packet if not already done
        const parsedPacket =
          optionalParsedPacket ?? parseIlpPacket(serializedPacket)

        switch (parsedPacket.type) {
          case IlpType.Prepare: {
            processPreparePacket({
              sourceEndpointInfo,
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
