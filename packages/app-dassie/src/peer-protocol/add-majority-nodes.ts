import { type Reactor, createActor } from "@dassie/lib-reactive"

import { MajorityNodeListSignal } from "./computed/majority-node-list"
import { NodeTableStore } from "./stores/node-table"

export const AddMajorityNodesActor = (reactor: Reactor) => {
  const nodeTableStore = reactor.use(NodeTableStore)

  return createActor((sig) => {
    const majorityNodes = sig.readAndTrack(MajorityNodeListSignal)

    for (const nodeId of majorityNodes) {
      if (nodeTableStore.read().has(nodeId)) {
        continue
      }

      nodeTableStore.act.addNode(nodeId)
    }
  })
}
