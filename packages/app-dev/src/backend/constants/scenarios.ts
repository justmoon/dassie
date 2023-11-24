import { NodeSettings, Scenario } from "../stores/scenario"

export const builtinScenarios = {
  sixAutopeeringStubNodes: {
    id: "sixAutopeeringStubNodes",
    name: "Six Autopeering Nodes (Stub Settlement)",
    environment: {},
    nodes: Array.from<NodeSettings>({ length: 6 }).fill({}),
  },
  twoPeerXrpl: {
    id: "twoPeerXrpl",
    name: "Two Peered Nodes (XRP Ledger Testnet Settlement)",
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
    id: "crossCurrency",
    name: "Cross-Currency Settlement (Stub and XRP Ledger Testnet)",
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
