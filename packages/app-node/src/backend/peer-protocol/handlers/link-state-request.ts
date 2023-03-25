import { createActor } from "@dassie/lib-reactive"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import { configSignal } from "../../config"
import type { IncomingPeerMessageEvent } from "../actions/handle-peer-message"
import { nodeTableStore } from "../stores/node-table"

export const handleLinkStateRequest = () =>
  createActor((sig) => {
    const nodeTable = sig.use(nodeTableStore)
    const { nodeId } = sig.getKeys(configSignal, ["nodeId"])

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

      if (!ownNodeTableEntry?.lastLinkStateUpdate) return EMPTY_UINT8ARRAY

      return ownNodeTableEntry.lastLinkStateUpdate
    }
  })
