import { type Reactor, createComputed } from "@dassie/lib-reactive"

import { EnvironmentConfig } from "../../config/environment-config"
import { MAJORITY_THRESHOLD } from "../../registration-client/constants/threshold"
import { compareSets } from "../../utils/compare-sets"
import { BootstrapNodeListsSignal } from "../signals/bootstrap-node-lists"
import type { NodeId } from "../types/node-id"

export const MajorityNodeListSignal = (reactor: Reactor) => {
  const bootstrapNodeListsSignal = reactor.use(BootstrapNodeListsSignal)
  const environmentConfig = reactor.use(EnvironmentConfig)

  return createComputed(
    reactor,
    (sig) => {
      const bootstrapNodes = environmentConfig.bootstrapNodes.map(
        ({ id }) => id,
      )
      const bootstrapNodeLists = sig.readAndTrack(bootstrapNodeListsSignal)

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
          .filter(
            ([, count]) => count / bootstrapNodes.length > MAJORITY_THRESHOLD,
          )
          .map(([nodeId]) => nodeId),
      )

      return majorityNodes
    },
    { comparator: compareSets },
  )
}
