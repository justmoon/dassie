import { createActor } from "@dassie/lib-reactive"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import { nodeIdSignal } from "../../ilp-connector/computed/node-id"
import type { IncomingPeerMessageEvent } from "../actions/handle-peer-message"
import { nodeTableStore } from "../stores/node-table"

export const handleLinkStateRequest = () =>
  createActor((sig) => {
    const nodeTable = sig.use(nodeTableStore)
    const nodeId = sig.get(nodeIdSignal)

    return ({
      message: {
        content: {
          value: {
            value: { subnetId },
          },
        },
      },
    }: IncomingPeerMessageEvent<"linkStateRequest">) => {
      const ownNodeTableEntry = nodeTable.read().get(`${subnetId}.${nodeId}`)

      if (!ownNodeTableEntry?.linkState.lastUpdate) return EMPTY_UINT8ARRAY

      return ownNodeTableEntry.linkState.lastUpdate
    }
  })
