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
        settlementMethods: ["xrpl-testnet"],
      },
      {
        settlementMethods: ["xrpl-testnet"],
      },
    ],
  },
  crossCurrency: {
    environment: {},
    nodes: [
      {
        settlementMethods: ["stub"],
      },
      {
        settlementMethods: ["stub", "xrpl-testnet"],
      },
      {
        settlementMethods: ["xrpl-testnet"],
      },
    ],
  },
} as const satisfies Record<string, Scenario>
