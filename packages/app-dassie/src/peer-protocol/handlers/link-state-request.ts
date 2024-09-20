import type { Reactor } from "@dassie/lib-reactive"

import type { PeerMessageHandler } from "../functions/handle-peer-message"
import { NodeTableStore } from "../stores/node-table"

export const HandleLinkStateRequest = ((reactor: Reactor) => {
  const nodeTableStore = reactor.use(NodeTableStore)

  return ({
    message: {
      content: {
        value: {
          value: { nodeIds },
        },
      },
    },
  }) => {
    const nodeTable = nodeTableStore.read()

    return nodeIds.map((nodeId) => {
      const entry = nodeTable.get(nodeId)
      return entry?.linkState ?
          {
            type: "found" as const,
            value: { bytes: entry.linkState.lastUpdate },
          }
        : { type: "notFound" as const, value: undefined }
    })
  }
}) satisfies PeerMessageHandler<"linkStateRequest">
