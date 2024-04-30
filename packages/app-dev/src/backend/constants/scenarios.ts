import { NodeSettings, Scenario } from "../stores/scenario"

export const builtinScenarios = {
  sixAutopeeringStubNodes: {
    id: "sixAutopeeringStubNodes",
    name: "Six Autopeering Nodes (Stub Settlement)",
    environment: {},
    nodes: Array.from<NodeSettings>({ length: 6 }).fill({}),
    defaultNodeSettings: {},
  },
  twoPeerXrpl: {
    id: "twoPeerXrpl",
    name: "Two Peered Nodes (XRP Ledger Testnet Settlement)",
    environment: {},
    nodes: [
      {
        settlementMethods: ["xrpl-testnet", "stub"],
      },
      {
        settlementMethods: ["xrpl-testnet", "stub"],
      },
    ],
    defaultNodeSettings: {
      settlementMethods: ["xrpl-testnet", "stub"],
    },
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
        settlementMethods: ["stub", "xrpl-testnet"],
      },
    ],
    defaultNodeSettings: {},
  },
} as const satisfies Record<string, Scenario>
