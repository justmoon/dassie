import { Reactor } from "@dassie/lib-reactive"

import { PeerMessageHandler } from "../actors/handle-peer-message"
import { NodeTableStore } from "../stores/node-table"

export const HandleLinkStateRequest = ((reactor: Reactor) => {
  const nodeTableStore = reactor.use(NodeTableStore)

  return ({
    message: {
      content: {
        value: {
          value: { nodeId },
        },
      },
    },
  }) => {
    const ownNodeTableEntry = nodeTableStore.read().get(nodeId)

    if (!ownNodeTableEntry?.linkState) {
      throw new Error("No own link state available")
    }

    return { bytes: ownNodeTableEntry.linkState.lastUpdate }
  }
}) satisfies PeerMessageHandler<"linkStateRequest">
