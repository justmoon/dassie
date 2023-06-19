import { ActorContext } from "@dassie/lib-reactive"
import { UnreachableCaseError } from "@dassie/lib-type-utils"

import { nodeTableStore } from "../.."
import { IlpDestinationInfo } from "./send-packet"

export const createGetLedgerPathForDestination = (sig: ActorContext) => {
  const nodeTable = sig.use(nodeTableStore)
  return (destination: IlpDestinationInfo): string => {
    switch (destination.type) {
      case "peer": {
        const nextHop = destination.firstHopOptions[0]!
        const peerState = nodeTable.read().get(nextHop)?.peerState

        if (peerState?.id !== "peered") {
          throw new Error(`Next hop node is not actually peered ${nextHop}`)
        }

        return `${peerState.subnetId}/peer/${nextHop}/interledger`
      }
      case "ildcp": {
        throw new Error("ILDCP does not have a ledger path")
      }
      case "btp": {
        return "builtin/owner/btp"
      }
      case "plugin": {
        return "builtin/owner/spsp"
      }
      default: {
        throw new UnreachableCaseError(destination)
      }
    }
  }
}
