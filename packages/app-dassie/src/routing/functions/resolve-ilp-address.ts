import type { Reactor } from "@dassie/lib-reactive"
import { Failure, UnreachableCaseError } from "@dassie/lib-type-utils"

import { IlpAllocationSchemeSignal } from "../../config/computed/ilp-allocation-scheme"
import type { EndpointInfo } from "../../ilp-connector/functions/send-packet"
import type { PeerEndpointInfo } from "../../ilp-connector/senders/send-peer-packets"
import { NodeTableStore } from "../../peer-protocol/stores/node-table"
import { GetLedgerIdForSettlementScheme } from "../../settlement-schemes/functions/get-ledger-id"
import { RoutingTableSignal } from "../signals/routing-table"

export default class NoRouteFoundFailure extends Failure {
  readonly name = "NoRouteFoundFailure"
}

export const NO_ROUTE_FOUND_FAILURE = new NoRouteFoundFailure()

export const ResolveIlpAddress = (reactor: Reactor) => {
  const routingTable = reactor.use(RoutingTableSignal)
  const nodeTable = reactor.use(NodeTableStore)
  const ilpAllocationScheme = reactor.use(IlpAllocationSchemeSignal)
  const getLedgerIdForSettlementScheme = reactor.use(
    GetLedgerIdForSettlementScheme,
  )

  return (ilpAddress: string): EndpointInfo | NoRouteFoundFailure => {
    const routingInfo = routingTable.read().lookup(ilpAddress)

    if (!routingInfo) {
      return NO_ROUTE_FOUND_FAILURE
    }

    switch (routingInfo.type) {
      case "fixed": {
        return routingInfo.destination
      }
      case "peer": {
        const nextHop = routingInfo.firstHopOptions[0]!
        const peerState = nodeTable.read().get(nextHop)?.peerState

        if (peerState?.id !== "peered") {
          throw new Error(`Next hop node is not actually peered ${nextHop}`)
        }

        const ledgerId = getLedgerIdForSettlementScheme(
          peerState.settlementSchemeId,
        )

        const destinationInfo: PeerEndpointInfo = {
          type: "peer",
          nodeId: nextHop,
          accountPath: `${ledgerId}:assets/interledger/${nextHop}`,
          ilpAddress: `${ilpAllocationScheme.read()}.das.${nextHop}`,
        }

        return destinationInfo
      }
      default: {
        throw new UnreachableCaseError(routingInfo)
      }
    }
  }
}
