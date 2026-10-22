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
  /**
   * What the fake ledger's issuers mint each node once its trust line
   * exists: a token and its value in dollars, such as RLUSD `"100"`. Only
   * the fake ledger mints on demand, so such a scenario needs it.
   */
  readonly mint?: { readonly token: IssuedToken; readonly value: string };
}

/** The dollar tokens the fake ledger issues, by their currency codes. */
export type IssuedToken = "RLUSD" | "DSUSD";

/** The settlement schemes on the XRP Ledger: their nodes' account is on
 * the testnet, or on the fake ledger. */
export const XRPL_SCHEMES: readonly string[] = [
  "xrpl-testnet",
  "xrpl-testnet-rlusd",
  "xrpl-testnet-dsusd",
];

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
    id: "rlusd-settlement",
    name: "RLUSD Settlement",
    description:
      "Network with two nodes settling in test RLUSD on the XRP Ledger, minted by the fake ledger's issuer",
    schemes: [["xrpl-testnet-rlusd"], ["xrpl-testnet-rlusd"]],
    addedSchemes: ["xrpl-testnet-rlusd"],
    bootstrap: [0, 1],
    mint: { token: "RLUSD", value: "100" },
  },
  {
    id: "dsusd-settlement",
    name: "DSUSD Settlement",
    description:
      "Network with two nodes settling in test DSUSD on the XRP Ledger, minted by the fake ledger's issuer",
    schemes: [["xrpl-testnet-dsusd"], ["xrpl-testnet-dsusd"]],
    addedSchemes: ["xrpl-testnet-dsusd"],
    bootstrap: [0, 1],
    mint: { token: "DSUSD", value: "100" },
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

/** Whether `def` settles on the XRP Ledger, with any of its nodes. */
export function settlesOnXrpl(def: ScenarioDef): boolean {
  return [...def.schemes, def.addedSchemes].some((schemes) =>
    schemes.some((scheme) => XRPL_SCHEMES.includes(scheme))
  );
}

/** Why `def` cannot run, with the fake XRP Ledger or without it. */
export function unavailable(
  def: ScenarioDef,
  fakeLedger: boolean,
): string | undefined {
  if (def.unavailable) return def.unavailable;
  if (def.mint !== undefined && !fakeLedger) {
    return `needs --xrpl fake: only the fake ledger's issuer mints test ${def.mint.token} on demand`;
  }
  return undefined;
}
