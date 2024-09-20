import type { Reactor } from "@dassie/lib-reactive"

import { StartNode } from "../functions/start-node"
import { generateNodeConfig } from "../utils/generate-node-config"
import type { StartScenarioParameters } from "./common"

export const name = "Two Simple Nodes"
export const description =
  "Network with two nodes using default configuration and stub settlement"

export const StartScenario = (reactor: Reactor) => {
  const startNode = reactor.use(StartNode)
  const ENVIRONMENT = {}

  return async ({ context }: StartScenarioParameters) => {
    const node1 = generateNodeConfig(0, {}, ENVIRONMENT)
    await startNode({ context, node: node1 })

    const node2 = generateNodeConfig(1, {}, ENVIRONMENT)
    await startNode({ context, node: node2 })
  }
}
