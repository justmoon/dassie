import { createActor } from "@dassie/lib-reactive"

import { nodeListResponse } from "../peer-schema"
import { NodeTableStore } from "../stores/node-table"

export const HandleNodeListRequestActor = () =>
  createActor((sig) => {
    const nodeTable = sig.use(NodeTableStore)

    return sig.handlers({
      handle: () => {
        const nodeLinkStates: { bytes: Uint8Array }[] = []

        for (const node of nodeTable.read().values()) {
          if (node.linkState?.lastUpdate) {
            nodeLinkStates.push({
              bytes: node.linkState.lastUpdate,
            })
          }
        }

        const nodeList = nodeListResponse.serializeOrThrow(nodeLinkStates)

        return nodeList
      },
    })
  })
