import type { Reactor } from "@dassie/lib-reactive"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import { configSignal } from "../../config"
import type { PeerMessageContent } from "../actions/handle-peer-message"
import { nodeTableStore } from "../stores/node-table"

export const handleLinkStateRequest = (reactor: Reactor) => {
  const nodeTable = reactor.use(nodeTableStore)
  const { nodeId } = reactor.use(configSignal).read()

  return ({ subnetId }: PeerMessageContent<"linkStateRequest">) => {
    const ownNodeTableEntry = nodeTable.read().get(`${subnetId}.${nodeId}`)

    if (!ownNodeTableEntry?.lastLinkStateUpdate) return EMPTY_UINT8ARRAY

    return ownNodeTableEntry.lastLinkStateUpdate
  }
}
