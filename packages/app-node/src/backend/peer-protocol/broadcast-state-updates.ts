import { createActor } from "@dassie/lib-reactive"
import { tell } from "@dassie/lib-type-utils"

import type { DassieReactor } from "../base/types/dassie-base"
import { NodeIdSignal } from "../ilp-connector/computed/node-id"
import { SendPeerMessage } from "./functions/send-peer-message"
import { NodeTableStore } from "./stores/node-table"

export const BroadcastStateUpdatesActor = (reactor: DassieReactor) => {
  const sendPeerMessage = reactor.use(SendPeerMessage)
  const nodeIdSignal = reactor.use(NodeIdSignal)

  return createActor((sig) => {
    const ownNodeId = nodeIdSignal.read()

    const linkStateUpdate = sig.readAndTrack(
      NodeTableStore,
      (nodeTable) => nodeTable.get(ownNodeId)?.linkState?.lastUpdate,
    )

    if (!linkStateUpdate) {
      return
    }

    const nodeIds = sig.read(NodeTableStore).keys()

    for (const nodeId of nodeIds) {
      tell(() =>
        sendPeerMessage({
          destination: nodeId,
          message: {
            type: "linkStateUpdate",
            value: {
              bytes: linkStateUpdate,
            },
          },
        }),
      )
    }
  })
}
