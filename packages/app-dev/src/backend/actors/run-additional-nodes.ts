import { setTimeout } from "node:timers/promises"

import { Reactor, createActor, createMapped } from "@dassie/lib-reactive"

import { StartNode } from "../functions/start-node"
import { children as logger } from "../logger/instances"
import { AdditionalNodesStore } from "../stores/additional-nodes"
import { EnvironmentStore } from "../stores/environment"
import { generateNodeConfig } from "../utils/generate-node-config"

// Amount of time to wait between starting each node process
const NODE_STARTUP_INTERVAL = 500

export interface NodeDefinition<T> {
  id: string
  port: number
  debugPort: number
  peers: string[]
  config: T
  url: string
  entry?: string
}

export const RunAdditionalNodesActor = (reactor: Reactor) => {
  const environmentStore = reactor.use(EnvironmentStore)
  const startNode = reactor.use(StartNode)

  return createActor(async (sig) => {
    // eslint-disable-next-line unicorn/consistent-function-scoping
    const NodeActorsMapped = (reactor: Reactor) =>
      createMapped(reactor, AdditionalNodesStore, (nodeIndex) =>
        createActor(async (sig) => {
          logger.info("starting additional node", { nodeIndex })
          const environmentSettings = environmentStore.read()

          const node = generateNodeConfig(nodeIndex, {}, environmentSettings)

          await startNode({ node, context: sig })

          await setTimeout(NODE_STARTUP_INTERVAL)
        }),
      )

    try {
      await sig.runMapSequential(NodeActorsMapped)
    } catch (error) {
      logger.error("error starting additional nodes", { error })
    }
  })
}
