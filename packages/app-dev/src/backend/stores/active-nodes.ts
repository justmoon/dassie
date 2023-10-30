import { enableMapSet, produce } from "immer"

import { createStore } from "@dassie/lib-reactive"

enableMapSet()

const INITIAL_NODES = ["d1", "d2", "d3", "d4", "d5", "d6"]

export const ActiveNodesStore = () =>
  createStore(new Set<string>(INITIAL_NODES), {
    addNode: () =>
      produce((nodes) => {
        let index = 1
        while (nodes.has(`d${index}`)) index++
        nodes.add(`d${index}`)
      }),
  })
