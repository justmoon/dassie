import { enableMapSet, produce } from "immer"

import { createStore } from "@dassie/lib-reactive"

import { NodeId } from "../types/node-id"

enableMapSet()

export const nodeDiscoveryQueueStore = () =>
  createStore(new Map<NodeId, NodeId>(), {
    addNode: (nodeId: NodeId, referrer: NodeId) =>
      produce((queue) => {
        if (!queue.has(nodeId)) {
          queue.set(nodeId, referrer)
        }
      }),
    removeNode: (nodeId: NodeId) =>
      produce((queue) => {
        queue.delete(nodeId)
      }),
  })
