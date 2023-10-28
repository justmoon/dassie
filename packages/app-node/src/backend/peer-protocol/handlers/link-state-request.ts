import { Reactor } from "@dassie/lib-reactive"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import { NodeIdSignal } from "../../ilp-connector/computed/node-id"
import { PeerMessageHandler } from "../actors/handle-peer-message"
import { NodeTableStore } from "../stores/node-table"

export const HandleLinkStateRequest = ((reactor: Reactor) => {
  const nodeTableStore = reactor.use(NodeTableStore)
  const nodeIdSignal = reactor.use(NodeIdSignal)

  return () => {
    const ownNodeTableEntry = nodeTableStore.read().get(nodeIdSignal.read())

    if (!ownNodeTableEntry?.linkState) return EMPTY_UINT8ARRAY

    return ownNodeTableEntry.linkState.lastUpdate
  }
}) satisfies PeerMessageHandler<"linkStateRequest">
