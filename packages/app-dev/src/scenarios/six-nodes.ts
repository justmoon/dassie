import type { Reactor } from "@dassie/lib-reactive"

import { StartNode } from "../functions/start-node"
import type { EnvironmentSettings } from "../stores/environment"
import { generateNodeConfig } from "../utils/generate-node-config"
import type { StartScenarioParameters } from "./common"

const NODE_COUNT = 6

export const name = "Six Simple Nodes"
export const description =
  "Network with six nodes using default configuration and stub settlement"

export const StartScenario = (reactor: Reactor) => {
  const startNode = reactor.use(StartNode)
  const ENVIRONMENT: Partial<EnvironmentSettings> = {
    additionalNodeStartIndex: NODE_COUNT,
    registeredNodes: Array.from({ length: NODE_COUNT }, (_, index) => index),
  }

  return async ({ context }: StartScenarioParameters) => {
    for (let index = 0; index < 6; index++) {
      const node = generateNodeConfig(index, {}, ENVIRONMENT)
      await startNode({ context, node })
    }
  }
}
