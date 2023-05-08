import { enableMapSet, produce } from "immer"

import { createStore } from "@dassie/lib-reactive"

import type { NodeTableKey } from "./node-table"

enableMapSet()

export const nodeDiscoveryQueueStore = () =>
  createStore(new Map<NodeTableKey, string>(), {
    addNode: (nodeKey: NodeTableKey, referrer: string) =>
      produce((queue) => {
        if (!queue.has(nodeKey)) {
          queue.set(nodeKey, referrer)
        }
      }),
    removeNode: (nodeKey: NodeTableKey) =>
      produce((queue) => {
        queue.delete(nodeKey)
      }),
  })
