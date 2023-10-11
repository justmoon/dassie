import { createActor } from "@dassie/lib-reactive"
import { UnreachableCaseError } from "@dassie/lib-type-utils"

import { ledgerStore } from "../accounting/stores/ledger"
import { IlpAllocationSchemeSignal } from "../config/computed/ilp-allocation-scheme"
import { connector as logger } from "../logger/instances"
import { NodeTableStore } from "../peer-protocol/stores/node-table"
import { createResolveIlpAddress } from "../routing/functions/resolve-ilp-address"
import { RoutingTableSignal } from "../routing/signals/routing-table"
import { NodeIlpAddressSignal } from "./computed/node-ilp-address"
import { createProcessFulfillPacket } from "./functions/process-fulfill-packet"
import { createProcessPreparePacket } from "./functions/process-prepare-packet"
import { createProcessRejectPacket } from "./functions/process-reject-packet"
import { createScheduleTimeout } from "./functions/schedule-timeout"
import { EndpointInfo, createPacketSender } from "./functions/send-packet"
import { createTriggerRejection } from "./functions/trigger-rejection"
import { IlpPacket, IlpType, parseIlpPacket } from "./schemas/ilp-packet-codec"
import { RequestIdMapSignal } from "./signals/request-id-map"
import { PreparedIlpPacketTopic } from "./topics/prepared-ilp-packet"
import { ResolvedIlpPacketTopic } from "./topics/resolved-ilp-packet"

export interface ProcessIncomingPacketParameters {
  sourceEndpointInfo: EndpointInfo
  serializedPacket: Uint8Array
  parsedPacket?: IlpPacket | undefined
  requestId: number
}

export const ProcessPacketActor = () =>
  createActor((sig) => {
    const ledger = sig.use(ledgerStore)
    const preparedIlpPacketTopic = sig.use(PreparedIlpPacketTopic)
    const resolvedIlpPacketTopic = sig.use(ResolvedIlpPacketTopic)
    const requestIdMap = sig.use(RequestIdMapSignal)
    const routingTable = sig.use(RoutingTableSignal)
    const nodeTable = sig.use(NodeTableStore)
    const ilpAllocationScheme = sig.use(IlpAllocationSchemeSignal)
    const ownIlpAddress = sig.use(NodeIlpAddressSignal)

    const resolveIlpAddress = createResolveIlpAddress({
      routingTable,
      nodeTable,
      ilpAllocationScheme,
    })
    const sendPacket = createPacketSender(sig)

    const processFulfillPacket = createProcessFulfillPacket({
      ledger,
      resolvedIlpPacketTopic,
      requestIdMap,
      sendPacket,
    })
    const processRejectPacket = createProcessRejectPacket({
      ledger,
      resolvedIlpPacketTopic,
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
      preparedIlpPacketTopicValue: preparedIlpPacketTopic,
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
