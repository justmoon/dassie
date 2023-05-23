import { enableMapSet, produce } from "immer"

import { createStore } from "@dassie/lib-reactive"

import type { NodeTableKey } from "./node-table"

enableMapSet()

export const nodeDiscoveryQueueStore = () =>
  createStore(new Map<NodeTableKey, string>(), {
    addNode: (nodeId: NodeTableKey, referrer: string) =>
      produce((queue) => {
        if (!queue.has(nodeId)) {
          queue.set(nodeId, referrer)
        }
      }),
    removeNode: (nodeId: NodeTableKey) =>
      produce((queue) => {
        queue.delete(nodeId)
      }),
  })
