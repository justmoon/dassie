import { createActor } from "@dassie/lib-reactive"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import { NodeIdSignal } from "../../ilp-connector/computed/node-id"
import { NodeTableStore } from "../stores/node-table"

export const HandleLinkStateRequestActor = () =>
  createActor((sig) => {
    const nodeTable = sig.use(NodeTableStore)
    const nodeId = sig.get(NodeIdSignal)

    return {
      handle: () => {
        const ownNodeTableEntry = nodeTable.read().get(nodeId)

        if (!ownNodeTableEntry?.linkState.lastUpdate) return EMPTY_UINT8ARRAY

        return ownNodeTableEntry.linkState.lastUpdate
      },
    }
  })
