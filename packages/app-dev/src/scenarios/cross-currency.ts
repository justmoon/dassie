import { StartNode } from "../functions/start-node"
import { EnvironmentStore } from "../stores/environment"
import type { DevelopmentReactor } from "../types/development-base"
import { generateNodeConfig } from "../utils/generate-node-config"
import type { StartScenarioParameters } from "./common"

export const name = "Cross-currency Settlement"
export const description =
  "Two nodes using different settlement methods (stub and XRPL testnet) and a third node supporting both"

export const StartScenario = (reactor: DevelopmentReactor) => {
  const environmentStore = reactor.use(EnvironmentStore)
  const startNode = reactor.use(StartNode)

  return async ({ context }: StartScenarioParameters) => {
    environmentStore.act.applyEnvironment({
      defaultNodeSettings: {},
      additionalNodeStartIndex: 3,
      registeredNodes: [0, 1, 2],
    })

    const node1 = generateNodeConfig(
      0,
      {
        settlementMethods: ["stub"],
      },
      environmentStore.read(),
    )
    await startNode({ context, node: node1 })

    const node2 = generateNodeConfig(
      1,
      {
        settlementMethods: ["stub", "xrpl-testnet"],
      },
      environmentStore.read(),
    )
    await startNode({ context, node: node2 })

    const node3 = generateNodeConfig(
      2,
      {
        settlementMethods: ["xrpl-testnet"],
      },
      environmentStore.read(),
    )
    await startNode({ context, node: node3 })
  }
}
