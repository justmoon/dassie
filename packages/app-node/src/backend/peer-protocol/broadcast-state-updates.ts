import { createActor } from "@dassie/lib-reactive"

import { NodeIdSignal } from "../ilp-connector/computed/node-id"
import { SendPeerMessageActor } from "./actors/send-peer-message"
import { NodeTableStore } from "./stores/node-table"

export const BroadcastStateUpdatesActor = () =>
  createActor((sig) => {
    const ownNodeId = sig.use(NodeIdSignal).read()

    const linkStateUpdate = sig.get(
      NodeTableStore,
      (nodeTable) => nodeTable.get(ownNodeId)?.linkState?.lastUpdate,
    )

    if (!linkStateUpdate) {
      return
    }

    const nodeIds = sig.use(NodeTableStore).read().keys()

    for (const nodeId of nodeIds) {
      sig.use(SendPeerMessageActor).api.send.tell({
        destination: nodeId,
        message: {
          type: "linkStateUpdate",
          value: {
            bytes: linkStateUpdate,
          },
        },
      })
    }
  })
