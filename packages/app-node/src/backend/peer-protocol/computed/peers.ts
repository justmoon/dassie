import { enableMapSet, produce } from "immer"

import { createComputed } from "@dassie/lib-reactive"
import { UnreachableCaseError } from "@dassie/lib-type-utils"

import { nodeTableStore } from "../stores/node-table"
import { NodeId } from "../types/node-id"

enableMapSet()

export const peersComputation = () =>
  createComputed((sig) => {
    const nodeTable = sig.use(nodeTableStore)

    const initialSet = new Set<NodeId>()

    for (const node of nodeTable.read().values()) {
      if (node.peerState.id === "peered") {
        initialSet.add(node.nodeId)
      }
    }

    nodeTable.changes.on(sig.reactor, ([actionId, parameters]) => {
      const peersComputationValue = sig.use(peersComputation)
      const peersSet = peersComputationValue.read()
      const priorSize = peersSet.size

      const newSet = produce(peersSet, (draft) => {
        switch (actionId) {
          case "addNode": {
            const { peerState, nodeId } = parameters[0]
            if (peerState.id === "peered") {
              draft.add(nodeId)
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
        peersComputationValue.write(newSet)
      }
    })

    return initialSet
  })

export const peersArrayComputation = () =>
  createComputed((sig) => [...sig.get(peersComputation)])
