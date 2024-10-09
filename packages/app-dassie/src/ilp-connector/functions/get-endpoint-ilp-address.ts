import { ILDCP_ADDRESS } from "@dassie/lib-protocol-ildcp"
import type { Reactor } from "@dassie/lib-reactive"
import { UnreachableCaseError } from "@dassie/lib-type-utils"

import { IlpAllocationSchemeSignal } from "../../config/computed/ilp-allocation-scheme"
import { MajorityNodeListSignal } from "../../peer-protocol/computed/majority-node-list"
import { NodeIlpAddressSignal } from "../computed/node-ilp-address"
import type { IlpAddress } from "../types/ilp-address"
import type { EndpointInfo } from "./send-packet"

export function GetEndpointIlpAddress(reactor: Reactor) {
  const ilpAllocationSchemeSignal = reactor.use(IlpAllocationSchemeSignal)
  const nodeIlpAddressSignal = reactor.use(NodeIlpAddressSignal)
  const majorityNodeListSignal = reactor.use(MajorityNodeListSignal)

  return function getEndpointIlpAddress(
    endpointInfo: EndpointInfo,
  ): IlpAddress {
    switch (endpointInfo.type) {
      case "peer": {
        return majorityNodeListSignal.read().has(endpointInfo.nodeId) ?
            `${ilpAllocationSchemeSignal.read()}.das.${endpointInfo.nodeId}`
          : `${nodeIlpAddressSignal.read()}.${endpointInfo.nodeId}`
      }
      case "ildcp": {
        return ILDCP_ADDRESS
      }
      case "btp": {
        return `${nodeIlpAddressSignal.read()}.${endpointInfo.connectionId}`
      }
      case "local": {
        return `${nodeIlpAddressSignal.read()}.${endpointInfo.localIlpAddressPart}`
      }
      case "http": {
        // TODO: Implement some valid address scheme
        return `test.not-implemented` as IlpAddress
      }
      default: {
        throw new UnreachableCaseError(endpointInfo)
      }
    }
  }
}
