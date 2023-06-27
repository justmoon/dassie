import { UnreachableCaseError } from "@dassie/lib-type-utils"
import { Failure } from "@dassie/lib-type-utils"

import { nodeTableStore } from "../.."
import { ilpAllocationSchemeSignal } from "../../config/computed/ilp-allocation-scheme"
import { EndpointInfo } from "../../ilp-connector/functions/send-packet"
import { PeerEndpointInfo } from "../../ilp-connector/senders/send-peer-packets"
import { routingTableSignal } from "../signals/routing-table"

export interface ResolveIlpAddressEnvironment {
  routingTable: ReturnType<typeof routingTableSignal>
  nodeTable: ReturnType<typeof nodeTableStore>
  ilpAllocationScheme: ReturnType<typeof ilpAllocationSchemeSignal>
}

export default class NoRouteFoundFailure extends Failure {
  readonly name = "NoRouteFoundFailure"
}

export const NO_ROUTE_FOUND_FAILURE = new NoRouteFoundFailure()

export const createResolveIlpAddress = ({
  routingTable,
  nodeTable,
  ilpAllocationScheme,
}: ResolveIlpAddressEnvironment) => {
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

        const destinationInfo: PeerEndpointInfo = {
          type: "peer",
          nodeId: nextHop,
          accountPath: `${peerState.subnetId}/peer/${nextHop}/interledger`,
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
