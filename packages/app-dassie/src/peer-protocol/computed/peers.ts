import { enableMapSet, produce } from "immer"

import { type Reactor, createSignal } from "@dassie/lib-reactive"
import { UnreachableCaseError } from "@dassie/lib-type-utils"

import { NodeTableStore } from "../stores/node-table"
import type { NodeId } from "../types/node-id"

enableMapSet()

export const PeersSignal = (reactor: Reactor) => {
  const nodeTable = reactor.use(NodeTableStore)

  const initialSet = new Set<NodeId>(
    // TODO: This conversion to an array will be unnecessary once we upgrade to
    // a Node.js version that supports iterator helpers.
    [...nodeTable.read().values()]
      .filter(({ peerState: { id } }) => id === "peered")
      .map(({ nodeId }) => nodeId),
  )

  const signal = createSignal(initialSet)

  nodeTable.changes.on(reactor, ([actionId, parameters]) => {
    signal.update(
      produce((draft) => {
        switch (actionId) {
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
          case "addNode":
          case "registerNode": {
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
