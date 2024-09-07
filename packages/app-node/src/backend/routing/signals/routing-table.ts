import { createSignal } from "@dassie/lib-reactive"

import type { EndpointInfo } from "../../ilp-connector/functions/send-packet"
import type { IlpAddress } from "../../ilp-connector/types/ilp-address"
import type { NodeId } from "../../peer-protocol/types/node-id"
import PrefixMap from "../utils/prefix-map"

export type RoutingInfo = FixedDestinationRoutingInfo | PeerRoutingInfo

export interface FixedDestinationRoutingInfo {
  type: "fixed"
  destination: EndpointInfo
}

export interface PeerRoutingInfo {
  type: "peer"
  firstHopOptions: NodeId[]
  distance: number
}

export const RoutingTableSignal = () =>
  createSignal(new PrefixMap<IlpAddress, RoutingInfo>(), {
    comparator: () => false,
  })
