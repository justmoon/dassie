import { createActor } from "@dassie/lib-reactive"

import { NodeTableKey, nodeTableStore } from "../stores/node-table"

export const peersComputation = () => {
  const peersSet = new Set<NodeTableKey>()
  const actor = createActor((sig) => {
    const nodeTable = sig.use(nodeTableStore)

    nodeTable.changes.on(([actionId, parameters]) => {
      const priorSize = peersSet.size

      switch (actionId) {
        case "addNode": {
          const { peerState, subnetId, nodeId } = parameters[0]
          if (peerState.id === "peered" || peerState.id === "request-peering") {
            peersSet.add(`${subnetId}.${nodeId}`)
          }
          break
        }
        case "updateNode": {
          const nodeKey = parameters[0]
          const { peerState } = parameters[1]
          if (
            peerState?.id === "peered" ||
            peerState?.id === "request-peering"
          ) {
            peersSet.add(nodeKey)
          } else if (peerState?.id === "none") {
            peersSet.delete(nodeKey)
          }
          break
        }
      }

      if (peersSet.size !== priorSize) {
        actor.emit(peersSet)
      }
    })

    return peersSet
  }, peersSet)

  return actor
}
