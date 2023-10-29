import { Reactor, createComputed } from "@dassie/lib-reactive"

import { EnvironmentConfigSignal } from "../../config/environment-config"
import { compareSets } from "../../utils/compare-sets"
import { BootstrapNodeListsSignal } from "../signals/bootstrap-node-lists"
import { NodeId } from "../types/node-id"

export const MajorityNodeListSignal = (reactor: Reactor) => {
  const bootstrapNodeListsSignal = reactor.use(BootstrapNodeListsSignal)
  const environmentConfigSignal = reactor.use(EnvironmentConfigSignal)

  return createComputed(
    reactor.lifecycle,
    (sig) => {
      const bootstrapNodes = environmentConfigSignal
        .read()
        .bootstrapNodes.map(({ id }) => id)
      const bootstrapNodeLists = sig.get(bootstrapNodeListsSignal)

      const voteCount = new Map<NodeId, number>()

      for (const bootstrapNodeId of bootstrapNodes) {
        const nodeList = bootstrapNodeLists.get(bootstrapNodeId)
        if (nodeList == null) {
          continue
        }

        for (const nodeId of nodeList.entries) {
          voteCount.set(nodeId, (voteCount.get(nodeId) ?? 0) + 1)
        }
      }

      const majorityNodes = new Set(
        [...voteCount.entries()]
          .filter(([, count]) => count > bootstrapNodes.length / 2)
          .map(([nodeId]) => nodeId),
      )

      return majorityNodes
    },
    { comparator: compareSets },
  )
}
