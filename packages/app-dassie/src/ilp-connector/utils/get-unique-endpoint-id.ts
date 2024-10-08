import { UnreachableCaseError } from "@dassie/lib-type-utils"

import type { EndpointInfo } from "../functions/send-packet"

export function getUniqueEndpointId(endpointInfo: EndpointInfo) {
  switch (endpointInfo.type) {
    case "peer": {
      return `peer:${endpointInfo.nodeId}`
    }
    case "ildcp": {
      return `ildcp`
    }
    case "btp": {
      return `btp:${endpointInfo.connectionId}`
    }
    case "local": {
      return `local:${endpointInfo.localIlpAddressPart}`
    }
    case "http": {
      return `http:${endpointInfo.id}`
    }
    default: {
      throw new UnreachableCaseError(endpointInfo)
    }
  }
}
