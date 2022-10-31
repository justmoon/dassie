import { createStore } from "@dassie/lib-reactive"

import { PEERS } from "../constants/development-nodes"
import { generateNodeConfig } from "../utils/generate-node-config"

export const activeNodesStore = () =>
  createStore(
    PEERS.map((peers, index) => generateNodeConfig(index, peers)),
    {
      addNode: (node: ReturnType<typeof generateNodeConfig>) => (nodes) =>
        [...nodes, node],
    }
  )
