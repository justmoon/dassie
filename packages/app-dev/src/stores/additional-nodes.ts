import { enableMapSet, produce } from "immer"

import { createStore } from "@dassie/lib-reactive"

enableMapSet()

export const AdditionalNodesStore = () =>
  createStore(new Set<number>()).actions({
    addNode: (nodeIndex: number) =>
      produce((draft) => {
        draft.add(nodeIndex)
      }),
    clear: () =>
      produce((draft) => {
        draft.clear()
      }),
  })
