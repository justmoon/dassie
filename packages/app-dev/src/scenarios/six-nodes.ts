import { StartNode } from "../functions/start-node"
import { EnvironmentStore } from "../stores/environment"
import type { DevelopmentReactor } from "../types/development-base"
import { generateNodeConfig } from "../utils/generate-node-config"
import type { StartScenarioParameters } from "./common"

const NODE_COUNT = 6

export const name = "Six Simple Nodes"
export const description =
  "Network with six nodes using default configuration and stub settlement"

export const StartScenario = (reactor: DevelopmentReactor) => {
  const environmentStore = reactor.use(EnvironmentStore)
  const startNode = reactor.use(StartNode)

  return async ({ context }: StartScenarioParameters) => {
    environmentStore.act.applyEnvironment({
      additionalNodeStartIndex: NODE_COUNT,
      registeredNodes: Array.from({ length: NODE_COUNT }, (_, index) => index),
    })

    for (let index = 0; index < 6; index++) {
      const node = generateNodeConfig(index, {}, environmentStore.read())
      await startNode({ context, node })
    }
  }
}
