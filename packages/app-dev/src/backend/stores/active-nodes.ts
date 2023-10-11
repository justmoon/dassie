import { enableMapSet, produce } from "immer"

import { createStore } from "@dassie/lib-reactive"

enableMapSet()

const INITIAL_NODES = ["n1", "n2", "n3", "n4", "n5", "n6"]

export const ActiveNodesStore = () =>
  createStore(new Set<string>(INITIAL_NODES), {
    addNode: () =>
      produce((nodes) => {
        let index = 1
        while (nodes.has(`n${index}`)) index++
        nodes.add(`n${index}`)
      }),
  })
