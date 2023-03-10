import { createStore } from "@dassie/lib-reactive"

import { PEERS } from "../constants/development-nodes"
import { generateNodeConfig } from "../utils/generate-node-config"
import {
  DEFAULT_ENVIRONMENT_SETTINGS,
  EnvironmentSettings,
} from "./environment-settings"

export const activeNodesStore = () =>
  createStore(
    PEERS.map((peers, index) =>
      generateNodeConfig(index, peers, DEFAULT_ENVIRONMENT_SETTINGS)
    ),
    {
      addNode: (node: ReturnType<typeof generateNodeConfig>) => (nodes) =>
        [...nodes, node],
      regenerateConfig:
        (environmentSettings: EnvironmentSettings) => (nodes) => {
          const newNodes = nodes.map((node, index) =>
            generateNodeConfig(index, node.peerIndices, environmentSettings)
          )

          return newNodes
        },
    }
  )
