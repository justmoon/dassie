import { NodeSettings, Scenario } from "../stores/scenario"

export const builtinScenarios = {
  sixAutopeeringStubNodes: {
    environment: {},
    nodes: Array.from<NodeSettings>({ length: 6 }).fill({}),
  },
  twoPeerXrpl: {
    environment: {},
    nodes: [
      {
        peers: [],
        settlementMethods: ["xrpl-testnet"],
      },
      {
        peers: [],
        settlementMethods: ["xrpl-testnet"],
      },
    ],
  },
} as const satisfies Record<string, Scenario>
