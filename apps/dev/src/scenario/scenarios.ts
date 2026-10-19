/**
 * The scenarios (spec/areas/dev.md#scenarios): how many nodes, which are
 * bootstrap nodes, and which settlement schemes they use. Nodes added later
 * join with the scenario's default schemes.
 *
 * @module
 */

/** Most nodes one dev environment runs, added ones included. */
export const MAX_NODES = 100;

export interface ScenarioDef {
  readonly id: string;
  readonly name: string;
  readonly description: string;
  /** The schemes of each node, by index; its length is the node count. */
  readonly schemes: readonly (readonly string[])[];
  /** The schemes of nodes added later. */
  readonly addedSchemes: readonly string[];
  /** The bootstrap nodes, by index. */
  readonly bootstrap: readonly number[];
  /** Why it cannot run yet, if it cannot. */
  readonly unavailable?: string;
}

export const SCENARIOS: readonly ScenarioDef[] = [
  {
    id: "two-nodes",
    name: "Two Simple Nodes",
    description:
      "Network with two nodes using default configuration and stub settlement",
    schemes: [["stub"], ["stub"]],
    addedSchemes: ["stub"],
    bootstrap: [0, 1],
  },
  {
    id: "six-nodes",
    name: "Six Simple Nodes",
    description:
      "Network with six nodes using default configuration and stub settlement",
    schemes: Array.from({ length: 6 }, () => ["stub"]),
    addedSchemes: ["stub"],
    bootstrap: [0, 1],
  },
  {
    id: "xrpl-settlement",
    name: "XRPL Testnet Settlement",
    description: "Network with two nodes using XRPL testnet settlement",
    schemes: [["xrpl-testnet"], ["xrpl-testnet"]],
    addedSchemes: ["xrpl-testnet"],
    bootstrap: [0, 1],
  },
  {
    id: "cross-currency",
    name: "Cross-currency Settlement",
    description:
      "Two nodes using different settlement methods (stub and XRPL testnet) and a third node supporting both",
    schemes: [["stub"], ["stub", "xrpl-testnet"], ["xrpl-testnet"]],
    addedSchemes: ["stub"],
    bootstrap: [0, 1],
  },
  {
    id: "one-node",
    name: "One Node",
    description: "A single node with stub settlement and no peers",
    schemes: [["stub"]],
    addedSchemes: ["stub"],
    bootstrap: [],
  },
];

/** The scenario the dev environment starts with. */
export const DEFAULT_SCENARIO = "two-nodes";

/** The scenario `id`, if there is one. */
export function findScenario(id: string): ScenarioDef | undefined {
  return SCENARIOS.find((scenario) => scenario.id === id);
}
