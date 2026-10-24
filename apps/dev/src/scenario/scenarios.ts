/**
 * The scenarios (spec/areas/dev.md#scenarios): how many nodes, which are
 * bootstrap nodes, which are periphery nodes, and which settlement schemes
 * they use. Nodes added later join as core nodes with the scenario's
 * default schemes.
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
   * What the fake ledger's issuers mint each node that runs the token's
   * scheme, once its trust line exists: a token and its value in dollars,
   * such as DSUSD `"100"`. Only the fake ledger mints on demand, so such a
   * scenario needs it.
   */
  readonly mint?: { readonly token: IssuedToken; readonly value: string };
  /**
   * Nodes set up without a hostname, so periphery nodes, by index, each
   * pinned to the core nodes it names (`DASSIE_UPLINKS`,
   * spec/areas/network.md#pinned-uplinks).
   */
  readonly periphery?: readonly Periphery[];
  /**
   * The fake ledger's pool of XRP and a token, which its issuer makes when
   * the scenario starts, priced at the dev rates. Only the fake ledger
   * makes one on demand, so such a scenario needs it.
   */
  readonly pool?: PoolDef;
}

/** A periphery node of a scenario, and the nodes it is pinned to. */
export interface Periphery {
  /** The node, by index. */
  readonly node: number;
  /** The core nodes it asks to be its uplinks, and no others, by index. */
  readonly uplinks: readonly number[];
}

/** A pool of XRP and a token in the fake ledger. */
export interface PoolDef {
  readonly token: IssuedToken;
  /**
   * The token it holds, in dollars, such as `"50000"`; it holds XRP worth
   * as much at the dev rates ({@linkcode poolDrops}).
   */
  readonly value: string;
  /** Its trading fee, in hundred-thousandths, as the ledger counts it. */
  readonly tradingFee: number;
}

/** The dollar tokens the fake ledger's testnet issues, by their currency
 * codes: DSUSD, the testnet's stand-in for RLUSD, whose test token's scheme
 * is disabled (spec/decisions/issued-assets.md). */
export type IssuedToken = "DSUSD";

/** The settlement scheme that settles in each token. */
export const TOKEN_SCHEMES: Readonly<Record<IssuedToken, string>> = {
  DSUSD: "xrpl-testnet-dsusd",
};

/** The settlement schemes on the XRP Ledger: their nodes' account is on
 * the testnet, or on the fake ledger. */
export const XRPL_SCHEMES: readonly string[] = [
  "xrpl-testnet",
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
    id: "dsusd-settlement",
    name: "DSUSD Settlement",
    description:
      "Network with two nodes settling in test DSUSD on the XRP Ledger, minted by the fake ledger's issuer, beside the fake ledger's XRP/DSUSD pool at the dev rate, which keeps DSUSD on its peg",
    schemes: [["xrpl-testnet-dsusd"], ["xrpl-testnet-dsusd"]],
    addedSchemes: ["xrpl-testnet-dsusd"],
    bootstrap: [0, 1],
    mint: { token: "DSUSD", value: "100" },
    pool: { token: "DSUSD", value: "50000", tradingFee: 500 },
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
    id: "market-maker",
    name: "Market Maker",
    description:
      "A core node with XRP and DSUSD, most of its value in XRP, and a market maker's two periphery nodes pinned to it, one with XRP and one with DSUSD, beside the fake ledger's XRP/DSUSD pool at the dev rate",
    schemes: [
      ["xrpl-testnet", "xrpl-testnet-dsusd"],
      ["xrpl-testnet"],
      ["xrpl-testnet-dsusd"],
    ],
    addedSchemes: ["xrpl-testnet", "xrpl-testnet-dsusd"],
    bootstrap: [0],
    periphery: [{ node: 1, uplinks: [0] }, { node: 2, uplinks: [0] }],
    // The faucet's 1000 XRP are worth 500 dollars at the dev rates, so the
    // core node holds about a twentieth of its value in DSUSD.
    mint: { token: "DSUSD", value: "25" },
    // Deep, so that the market maker's swaps hardly move its price.
    pool: { token: "DSUSD", value: "50000", tradingFee: 500 },
  },
  {
    id: "market-maker-two-targets",
    name: "Market Maker, Two Targets",
    description:
      "Two core nodes with XRP and DSUSD, most of their value in XRP, and a market maker's two periphery nodes each pinned to both, one with XRP and one with DSUSD, beside the fake ledger's XRP/DSUSD pool at the dev rate",
    schemes: [
      ["xrpl-testnet", "xrpl-testnet-dsusd"],
      ["xrpl-testnet", "xrpl-testnet-dsusd"],
      ["xrpl-testnet"],
      ["xrpl-testnet-dsusd"],
    ],
    addedSchemes: ["xrpl-testnet", "xrpl-testnet-dsusd"],
    bootstrap: [0, 1],
    periphery: [{ node: 2, uplinks: [0, 1] }, { node: 3, uplinks: [0, 1] }],
    // As in "market-maker": each core node holds about a twentieth of its
    // value in DSUSD.
    mint: { token: "DSUSD", value: "25" },
    pool: { token: "DSUSD", value: "50000", tradingFee: 500 },
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

/** Node `index` of `def` as a periphery node, if it is one. */
export function peripheryOf(
  def: ScenarioDef,
  index: number,
): Periphery | undefined {
  return def.periphery?.find((entry) => entry.node === index);
}

/** Whether a node of `def` that runs `schemes` is minted its token. */
export function minted(def: ScenarioDef, schemes: readonly string[]): boolean {
  return def.mint !== undefined &&
    schemes.includes(TOKEN_SCHEMES[def.mint.token]);
}

/** The decimals of a millionth. */
const MICRO_DECIMALS = 6;
const MICROS = 10n ** BigInt(MICRO_DECIMALS);

/**
 * `text`, a decimal of at most six decimals such as `"0.92"`, in
 * millionths, exactly; undefined if it is no such decimal.
 */
export function micros(text: string): bigint | undefined {
  const match = /^(\d{1,15})(?:\.(\d{1,6}))?$/.exec(text);
  if (!match) return undefined;
  const fraction = (match[2] ?? "").padEnd(MICRO_DECIMALS, "0");
  return BigInt(match[1]) * MICROS + BigInt(fraction);
}

/**
 * The drops of XRP worth `value` dollars when a dollar buys `xrpPerDollar`
 * XRP, rounded down, so that a pool of both is priced at that rate.
 */
export function poolDrops(value: string, xrpPerDollar: string): bigint {
  const dollars = micros(value);
  const rate = micros(xrpPerDollar);
  if (dollars === undefined || rate === undefined) {
    throw new Error(`no pool of ${value} dollars at ${xrpPerDollar} XRP`);
  }
  // Millionths of a dollar times millionths of an XRP per dollar are
  // drops times a million.
  return dollars * rate / MICROS;
}

/**
 * What is wrong with `def`'s periphery nodes, if anything: each is one of
 * its nodes, once, and no bootstrap node, since a node list is served
 * only by core nodes; each is pinned to at least one core node of it, each
 * once. The node checks how many it may name (`DASSIE_UPLINKS`).
 */
export function invalid(def: ScenarioDef): string | undefined {
  const count = def.schemes.length;
  const inRange = (index: number) =>
    Number.isSafeInteger(index) && index >= 0 && index < count;
  const periphery = def.periphery ?? [];
  const nodes = new Set(periphery.map((entry) => entry.node));
  if (nodes.size !== periphery.length) return "a periphery node twice";
  for (const { node, uplinks } of periphery) {
    if (!inRange(node)) return `no node ${node}`;
    if (def.bootstrap.includes(node)) {
      return `periphery node ${node} is a bootstrap node`;
    }
    if (uplinks.length === 0) return `periphery node ${node} has no uplink`;
    if (new Set(uplinks).size !== uplinks.length) {
      return `periphery node ${node} names an uplink twice`;
    }
    const core = (uplink: number) => inRange(uplink) && !nodes.has(uplink);
    const stray = uplinks.find((uplink) => !core(uplink));
    if (stray !== undefined) {
      return `periphery node ${node} is pinned to ${stray}, no core node`;
    }
  }
  return undefined;
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
  if (def.pool !== undefined && !fakeLedger) {
    return `needs --xrpl fake: only the fake ledger's issuer makes a pool of test ${def.pool.token}`;
  }
  return undefined;
}
