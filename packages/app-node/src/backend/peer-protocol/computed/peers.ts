import produce, { enableMapSet } from "immer"

import { createActor, createComputed } from "@dassie/lib-reactive"
import { UnreachableCaseError } from "@dassie/lib-type-utils"

import { NodeTableKey, nodeTableStore } from "../stores/node-table"

enableMapSet()

export const peersComputation = () => {
  const emptySet = new Set<NodeTableKey>()
  const actor = createActor((sig) => {
    const nodeTable = sig.use(nodeTableStore)

    const initialSet = new Set<NodeTableKey>()

    for (const node of nodeTable.read().values()) {
      if (node.peerState.id === "peered") {
        initialSet.add(`${node.subnetId}.${node.nodeId}`)
      }
    }

    nodeTable.changes.on(([actionId, parameters]) => {
      const peersSet = actor.read()
      const priorSize = actor.read().size

      const newSet = produce(peersSet, (draft) => {
        switch (actionId) {
          case "addNode": {
            const { peerState, subnetId, nodeId } = parameters[0]
            if (peerState.id === "peered") {
              draft.add(`${subnetId}.${nodeId}`)
            }
            break
          }
          case "updateNode": {
            const nodeKey = parameters[0]
            const { peerState } = parameters[1]
            if (peerState?.id === "peered") {
              draft.add(nodeKey)
            } else if (peerState) {
              draft.delete(nodeKey)
            }
            break
          }
          default: {
            throw new UnreachableCaseError(actionId)
          }
        }
      })

      if (newSet.size !== priorSize) {
        actor.write(newSet)
      }
    })

    return initialSet
  }, emptySet)

  return actor
}

export const peersArrayComputation = () =>
  createComputed((sig) => [...sig.get(peersComputation)])
