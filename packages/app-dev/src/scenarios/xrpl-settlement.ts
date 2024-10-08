import { StartNode } from "../functions/start-node"
import { EnvironmentStore } from "../stores/environment"
import type { DevelopmentReactor } from "../types/development-base"
import { generateNodeConfig } from "../utils/generate-node-config"
import type { StartScenarioParameters } from "./common"

export const name = "XRPL Testnet Settlement"
export const description =
  "Network with two nodes using XRPL testnet settlement"

export const StartScenario = (reactor: DevelopmentReactor) => {
  const environmentStore = reactor.use(EnvironmentStore)
  const startNode = reactor.use(StartNode)

  return async ({ context }: StartScenarioParameters) => {
    environmentStore.act.applyEnvironment({
      defaultNodeSettings: {
        settlementMethods: ["xrpl-testnet"],
      },
      additionalNodeStartIndex: 2,
    })

    const node1 = generateNodeConfig(
      0,
      {
        settlementMethods: ["xrpl-testnet"],
      },
      environmentStore.read(),
    )
    await startNode({ context, node: node1 })

    const node2 = generateNodeConfig(
      1,
      {
        settlementMethods: ["xrpl-testnet"],
      },
      environmentStore.read(),
    )
    await startNode({ context, node: node2 })
  }
}
