import type { Reactor } from "@dassie/lib-reactive"

import { StartNode } from "../functions/start-node"
import type { EnvironmentSettings } from "../stores/environment"
import { generateNodeConfig } from "../utils/generate-node-config"
import type { StartScenarioParameters } from "./common"

export const name = "Cross-currency Settlement"
export const description =
  "Two nodes using different settlement methods (stub and XRPL testnet) and a third node supporting both"

export const StartScenario = (reactor: Reactor) => {
  const startNode = reactor.use(StartNode)
  const ENVIRONMENT: EnvironmentSettings = {
    defaultNodeSettings: {},
    additionalNodeStartIndex: 3,
  }

  return async ({ lifecycle }: StartScenarioParameters) => {
    const node1 = generateNodeConfig(
      0,
      {
        settlementMethods: ["stub"],
      },
      ENVIRONMENT,
    )
    await startNode({ lifecycle, node: node1 })

    const node2 = generateNodeConfig(
      1,
      {
        settlementMethods: ["stub", "xrpl-testnet"],
      },
      ENVIRONMENT,
    )
    await startNode({ lifecycle, node: node2 })

    const node3 = generateNodeConfig(
      2,
      {
        settlementMethods: ["xrpl-testnet"],
      },
      ENVIRONMENT,
    )
    await startNode({ lifecycle, node: node3 })
  }
}
