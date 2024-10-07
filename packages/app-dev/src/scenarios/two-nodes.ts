import { StartNode } from "../functions/start-node"
import type { DevelopmentReactor } from "../types/development-base"
import { generateNodeConfig } from "../utils/generate-node-config"
import type { StartScenarioParameters } from "./common"

export const name = "Two Simple Nodes"
export const description =
  "Network with two nodes using default configuration and stub settlement"

export const StartScenario = (reactor: DevelopmentReactor) => {
  const startNode = reactor.use(StartNode)

  return async ({ context }: StartScenarioParameters) => {
    const node1 = generateNodeConfig(0, {})
    await startNode({ context, node: node1 })

    const node2 = generateNodeConfig(1, {})
    await startNode({ context, node: node2 })
  }
}
