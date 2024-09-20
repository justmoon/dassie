import { castDraft, enableMapSet, produce } from "immer"

import { createStore } from "@dassie/lib-reactive"

import type { NodeConfig } from "../utils/generate-node-config"

enableMapSet()

export const ActiveNodesStore = () =>
  createStore(new Set<NodeConfig>()).actions({
    addNode: (node: NodeConfig) =>
      produce((draft) => {
        draft.add(castDraft(node))
      }),
    removeNode: (node: NodeConfig) =>
      produce((draft) => {
        draft.delete(castDraft(node))
      }),
  })
