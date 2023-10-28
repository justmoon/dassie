import { Reactor } from "@dassie/lib-reactive"

import { NodeIdSignal } from "../../ilp-connector/computed/node-id"
import { PeerMessageHandler } from "../actors/handle-peer-message"
import { NodeTableStore } from "../stores/node-table"

export const HandleLinkStateRequest = ((reactor: Reactor) => {
  const nodeTableStore = reactor.use(NodeTableStore)
  const nodeIdSignal = reactor.use(NodeIdSignal)

  return () => {
    const ownNodeTableEntry = nodeTableStore.read().get(nodeIdSignal.read())

    if (!ownNodeTableEntry?.linkState) {
      throw new Error("No own link state available")
    }

    return { bytes: ownNodeTableEntry.linkState.lastUpdate }
  }
}) satisfies PeerMessageHandler<"linkStateRequest">
