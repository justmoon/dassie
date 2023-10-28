import { Reactor } from "@dassie/lib-reactive"

import { PeerMessageHandler } from "../actors/handle-peer-message"
import { NodeTableStore } from "../stores/node-table"

export const HandleNodeListRequest = ((reactor: Reactor) => {
  const nodeTableStore = reactor.use(NodeTableStore)

  return () => {
    const nodeLinkStates: { bytes: Uint8Array }[] = []

    for (const node of nodeTableStore.read().values()) {
      if (node.linkState?.lastUpdate) {
        nodeLinkStates.push({
          bytes: node.linkState.lastUpdate,
        })
      }
    }

    return nodeLinkStates
  }
}) satisfies PeerMessageHandler<"nodeListRequest">
