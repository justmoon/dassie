import { Reactor, createActor } from "@dassie/lib-reactive"

import { MajorityNodeListSignal } from "./computed/majority-node-list"
import { ModifyNodeTable } from "./functions/modify-node-table"
import { NodeTableStore } from "./stores/node-table"

export const AddMajorityNodesActor = (reactor: Reactor) => {
  const nodeTableStore = reactor.use(NodeTableStore)
  const modifyNodeTable = reactor.use(ModifyNodeTable)

  return createActor((sig) => {
    const majorityNodes = sig.readAndTrack(MajorityNodeListSignal)

    for (const nodeId of majorityNodes) {
      if (nodeTableStore.read().has(nodeId)) {
        continue
      }

      modifyNodeTable.addNode(nodeId)
    }
  })
}
