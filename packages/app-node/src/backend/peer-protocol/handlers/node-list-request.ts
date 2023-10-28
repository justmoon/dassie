import { Reactor } from "@dassie/lib-reactive"

import { PeerMessageHandler } from "../actors/handle-peer-message"
import { nodeListResponse } from "../peer-schema"
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

    const nodeList = nodeListResponse.serializeOrThrow(nodeLinkStates)

    return nodeList
  }
}) satisfies PeerMessageHandler<"nodeListRequest">
