import { Reactor, createActor } from "@dassie/lib-reactive"

import { MajorityNodeListSignal } from "./computed/majority-node-list"
import { ModifyNodeTableActor } from "./modify-node-table"
import { NodeTableStore } from "./stores/node-table"

export const AddMajorityNodesActor = (reactor: Reactor) => {
  const nodeTableStore = reactor.use(NodeTableStore)
  const modifyNodeTableActor = reactor.use(ModifyNodeTableActor)

  return createActor((sig) => {
    const majorityNodes = sig.get(MajorityNodeListSignal)

    for (const nodeId of majorityNodes) {
      if (nodeTableStore.read().has(nodeId)) {
        continue
      }

      modifyNodeTableActor.api.addNode.tell(nodeId)
    }
  })
}
