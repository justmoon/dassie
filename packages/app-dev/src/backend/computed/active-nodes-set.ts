import { createComputed } from "@dassie/lib-reactive"

import { activeNodesStore } from "../stores/active-nodes"

export const activeNodesSetSignal = () =>
  createComputed((sig) => {
    const activeNodes = sig.get(activeNodesStore)
    const activeNodeIds = activeNodes.map((node) => node.id)
    return new Set(activeNodeIds)
  })
