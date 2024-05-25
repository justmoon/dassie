import type { Reactor } from "@dassie/lib-reactive"

import { StartNode } from "../functions/start-node"
import type { EnvironmentSettings } from "../stores/environment"
import { generateNodeConfig } from "../utils/generate-node-config"
import type { StartScenarioParameters } from "./common"

export const name = "XRPL Testnet Settlement"
export const description =
  "Network with two nodes using XRPL testnet settlement"

export const StartScenario = (reactor: Reactor) => {
  const startNode = reactor.use(StartNode)
  const ENVIRONMENT: EnvironmentSettings = {
    defaultNodeSettings: {
      settlementMethods: ["xrpl-testnet"],
    },
    additionalNodeStartIndex: 2,
  }

  return async ({ context }: StartScenarioParameters) => {
    const node1 = generateNodeConfig(
      0,
      {
        settlementMethods: ["xrpl-testnet"],
      },
      ENVIRONMENT,
    )
    await startNode({ context, node: node1 })

    const node2 = generateNodeConfig(
      1,
      {
        settlementMethods: ["xrpl-testnet"],
      },
      ENVIRONMENT,
    )
    await startNode({ context, node: node2 })
  }
}
