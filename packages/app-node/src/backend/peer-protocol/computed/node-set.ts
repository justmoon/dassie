import { enableMapSet, produce } from "immer"

import { type Reactor, createSignal } from "@dassie/lib-reactive"
import { UnreachableCaseError } from "@dassie/lib-type-utils"

import { NodeTableStore } from "../stores/node-table"
import type { NodeId } from "../types/node-id"

enableMapSet()

/**
 * A reactive set of the IDs of all nodes in the node table.
 */
export const NodeSetSignal = (reactor: Reactor) => {
  const nodeTable = reactor.use(NodeTableStore)

  const initialSet = new Set<NodeId>(nodeTable.read().keys())

  const signal = createSignal(initialSet)

  nodeTable.changes.on(reactor, ([actionId, parameters]) => {
    signal.update(
      produce((draft) => {
        switch (actionId) {
          case "addNode": {
            const { nodeId } = parameters[0]
            draft.add(nodeId)
            break
          }
          case "updateNode": {
            break
          }
          default: {
            throw new UnreachableCaseError(actionId)
          }
        }
      }),
    )
  })

  return signal
}
