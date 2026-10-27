/**
 * The scenarios (spec/areas/dev.md#scenarios): how many nodes, which are
 * bootstrap nodes, which are periphery nodes, which settlement schemes
 * they use, with which configs, their realm, and the settlement hub some
 * of them validate and the others join. Nodes added later join as core
 * nodes with the scenario's default schemes.
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
  /**
   * The realm its nodes run in: `test` unless it says `live`. Live nodes
   * run mainnet's schemes, by the node's dev switch, which only a build
   * with debug assertions honours (spec/decisions/mainnet-schemes.md), and
   * only against the fake ledger's mainnet, never with real money.
   */
  readonly realm?: "test" | "live";
  /**
   * Each scheme's config, by scheme ID, where it is not empty, for every
   * node that runs it: such as `{"peers":false}` for an XRP Ledger scheme
   * that is only its owner's book (spec/areas/settlement.md#owner-books).
   * The hub scheme's is the founded hub's rules, never set here.
   */
  readonly configs?: Readonly<Record<string, string>>;
  /**
   * A settlement hub on the fake ledger: the scenario founds it once its
   * nodes run, and its members join it once it is founded. Only the fake
   * ledger has it, so such a scenario needs it.
   */
  readonly hub?: HubDef;
  /**
   * Several settlement hubs on the fake ledger, in place of `hub`: each
   * founded and joined as `hub` is, of XRP or of `token`, one per asset,
   * with the scheme of its asset (`hubs.ts`).
   */
  readonly hubs?: readonly (HubDef & { readonly token?: IssuedToken })[];
}

/**
 * A settlement hub of a scenario (spec/areas/hubs.md#founding-a-hub): the
 * nodes that validate it, by a validator file that lists the sponsor; the
 * sponsor, which founds it through its owner's RPC and funds its escrow
 * from its owner book of XRP; and the members, which join it with its
 * rules and fund their accounts in it from their own owner books of XRP.
 * Nodes added later join it as members.
 */
export interface HubDef {
  /** The nodes that validate it, by index. */
  readonly validators: readonly number[];
  /** The node that founds it, by index. */
  readonly sponsor: number;
  /** The nodes that join it, by index. */
  readonly members: readonly number[];
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

/** The dollar tokens the fake ledger issues, by their currency codes:
 * DSUSD on its testnet, the stand-in for RLUSD, whose test token's scheme
 * is disabled (spec/decisions/issued-assets.md), and RLUSD on its mainnet.
 */
export type IssuedToken = "DSUSD" | "RLUSD";

/** The settlement scheme that settles in each token. */
export const TOKEN_SCHEMES: Readonly<Record<IssuedToken, string>> = {
  DSUSD: "xrpl-testnet-dsusd",
  RLUSD: "xrpl-rlusd",
};

/** The settlement schemes on the XRP Ledger: their nodes' account is on
 * the testnet, or on the fake ledger, its testnet or its mainnet. */
export const XRPL_SCHEMES: readonly string[] = [
  "xrpl-testnet-xrp",
  "xrpl-testnet-dsusd",
  "xrpl-xrp",
  "xrpl-rlusd",
];

/** The scheme of a member of the XRP hub on the testnet, and its ledger. */
export const HUB_SCHEME = "hub-xrpl-testnet-xrp";
export const HUB_LEDGER = "hub-xrpl-testnet-xrp+xrp";

/**
 * The owner book of the hub's asset: the XRP Ledger testnet scheme in XRP,
 * from which the sponsor funds the escrow and a member its account
 * (spec/areas/hubs.md#in-the-node).
 */
export const HUB_OWNER_BOOK = "xrpl-testnet-xrp";

/** The fewest and the most validators a hub is founded with
 * (`FoundHubRequest`). */
export const MIN_HUB_VALIDATORS = 4;
export const MAX_HUB_VALIDATORS = 16;

/**
 * What a member's hub scheme is set up with: the founded hub's rules, the
 * hex of their encoding, as `ListHubFoundings` shows them.
 */
export function hubConfig(rules: string): string {
  return JSON.stringify({ rules });
}

/**
 * The validator file of a node that validates the hubs `sponsor` founds,
 * following their escrows on the XRP Ledger testnet at `xrpl`
 * (spec/areas/hubs.md#the-validator-in-the-node): it lists no hub, since
 * the sponsor founds them with the node. It proves the escrows' ledgers
 * under the UNL of `publishers`, the fake ledger's, where given.
 */
export function validatorFile(
  sponsor: string,
  xrpl: string,
  publishers: Record<string, unknown> | null = null,
): string {
  return JSON.stringify({
    sponsors: [sponsor],
    hubs: [],
    xrpl,
    network: "testnet",
    ...publishers,
  });
}

/** Whether node `index` of `def` validates its hub. */
export function validates(def: ScenarioDef, index: number): boolean {
  return def.hub?.validators.includes(index) ?? false;
}

/** Whether node `index` of `def` is a member of its hub: one it names, or
 * one added later. */
export function joinsHub(def: ScenarioDef, index: number): boolean {
  const hub = def.hub;
  if (hub === undefined) return false;
  return hub.members.includes(index) || index >= def.schemes.length;
}

/**
 * What a live scenario deposits into each node's mainnet account, in drops,
 * since mainnet has no faucet: as much as the testnet faucet gives, 1000
 * XRP.
 */
export const MAINNET_DEPOSIT_DROPS = 1_000_000_000n;

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
    schemes: [["xrpl-testnet-xrp"], ["xrpl-testnet-xrp"]],
    addedSchemes: ["xrpl-testnet-xrp"],
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
    id: "xrpl-live-settlement",
    name: "XRPL Mainnet Settlement, Fake",
    description:
      "Network with two nodes in the live realm settling in XRP on the fake ledger's mainnet, by the dev switch; each node's account is funded by a deposit, as mainnet has no faucet",
    schemes: [["xrpl-xrp"], ["xrpl-xrp"]],
    addedSchemes: ["xrpl-xrp"],
    bootstrap: [0, 1],
    realm: "live",
  },
  {
    id: "rlusd-live-settlement",
    name: "RLUSD Mainnet Settlement, Fake",
    description:
      "Network with two nodes in the live realm settling in RLUSD on the fake ledger's mainnet, by the dev switch, minted by its RLUSD issuer, beside its XRP/RLUSD pool at the dev rate, which keeps RLUSD on its peg",
    schemes: [["xrpl-rlusd"], ["xrpl-rlusd"]],
    addedSchemes: ["xrpl-rlusd"],
    bootstrap: [0, 1],
    mint: { token: "RLUSD", value: "100" },
    // 200,000 XRP at the dev rate: deeper than RLUSD's floor of 100,000
    // XRP, below which its peg guard reads no price, even 5% off its peg.
    pool: { token: "RLUSD", value: "100000", tradingFee: 500 },
    realm: "live",
  },
  {
    id: "cross-currency",
    name: "Cross-currency Settlement",
    description:
      "Two nodes using different settlement methods (stub and XRPL testnet) and a third node supporting both",
    schemes: [["stub"], ["stub", "xrpl-testnet-xrp"], ["xrpl-testnet-xrp"]],
    addedSchemes: ["stub"],
    bootstrap: [0, 1],
  },
  {
    id: "market-maker",
    name: "Market Maker",
    description:
      "A core node with XRP and DSUSD, most of its value in XRP, and a market maker's two periphery nodes pinned to it, one with XRP and one with DSUSD, beside the fake ledger's XRP/DSUSD pool at the dev rate",
    schemes: [
      ["xrpl-testnet-xrp", "xrpl-testnet-dsusd"],
      ["xrpl-testnet-xrp"],
      ["xrpl-testnet-dsusd"],
    ],
    addedSchemes: ["xrpl-testnet-xrp", "xrpl-testnet-dsusd"],
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
      ["xrpl-testnet-xrp", "xrpl-testnet-dsusd"],
      ["xrpl-testnet-xrp", "xrpl-testnet-dsusd"],
      ["xrpl-testnet-xrp"],
      ["xrpl-testnet-dsusd"],
    ],
    addedSchemes: ["xrpl-testnet-xrp", "xrpl-testnet-dsusd"],
    bootstrap: [0, 1],
    periphery: [{ node: 2, uplinks: [0, 1] }, { node: 3, uplinks: [0, 1] }],
    // As in "market-maker": each core node holds about a twentieth of its
    // value in DSUSD.
    mint: { token: "DSUSD", value: "25" },
    pool: { token: "DSUSD", value: "50000", tradingFee: 500 },
  },
  {
    id: "hub",
    name: "Settlement Hub",
    description:
      "Four nodes validate a settlement hub that the first of them founds on the fake ledger, and two members join it, fund it from their own XRP and peer over it alone",
    schemes: [
      [HUB_OWNER_BOOK],
      [],
      [],
      [],
      [HUB_OWNER_BOOK, HUB_SCHEME],
      [HUB_OWNER_BOOK, HUB_SCHEME],
    ],
    addedSchemes: [HUB_OWNER_BOOK, HUB_SCHEME],
    bootstrap: [4, 5],
    configs: { [HUB_OWNER_BOOK]: JSON.stringify({ peers: false }) },
    hub: { validators: [0, 1, 2, 3], sponsor: 0, members: [4, 5] },
  },
  {
    id: "two-hubs",
    name: "XRP and DSUSD Hubs",
    description:
      "Four nodes validate an XRP hub and a DSUSD hub that the first of them founds on the fake ledger, beside its XRP/DSUSD pool at the dev rate; one member joins the XRP hub alone, one the DSUSD hub alone, and two join both, peer over both and convert between them, each funding its accounts from its own XRP and DSUSD",
    schemes: [
      [HUB_OWNER_BOOK],
      [],
      [],
      [],
      [HUB_OWNER_BOOK, HUB_SCHEME],
      [
        HUB_OWNER_BOOK,
        HUB_SCHEME,
        "xrpl-testnet-dsusd",
        "hub-xrpl-testnet-dsusd",
      ],
      [
        HUB_OWNER_BOOK,
        HUB_SCHEME,
        "xrpl-testnet-dsusd",
        "hub-xrpl-testnet-dsusd",
      ],
      [HUB_OWNER_BOOK, "xrpl-testnet-dsusd", "hub-xrpl-testnet-dsusd"],
    ],
    addedSchemes: [HUB_OWNER_BOOK, HUB_SCHEME],
    bootstrap: [5, 6],
    configs: {
      [HUB_OWNER_BOOK]: JSON.stringify({ peers: false }),
      "xrpl-testnet-dsusd": JSON.stringify({ peers: false }),
    },
    mint: { token: "DSUSD", value: "100" },
    pool: { token: "DSUSD", value: "50000", tradingFee: 500 },
    hubs: [
      { validators: [0, 1, 2, 3], sponsor: 0, members: [4, 5, 6] },
      {
        validators: [0, 1, 2, 3],
        sponsor: 0,
        members: [5, 6, 7],
        token: "DSUSD",
      },
    ],
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

/** Whether `def`'s nodes run in the live realm. */
export function live(def: ScenarioDef): boolean {
  return def.realm === "live";
}

/** The network of the fake ledger `def` settles on: mainnet for a live
 * scenario, else the testnet. */
export function ledgerNetwork(def: ScenarioDef): "testnet" | "mainnet" {
  return live(def) ? "mainnet" : "testnet";
}

/**
 * Why `def`'s nodes start from scratch on every run, a restart too, if
 * they do; the fake ledger is a new chain on every run. A live scenario's
 * because mainnet never resets, so a mainnet account that knew an earlier
 * chain stops (spec/areas/xrpl.md#real-money). A hub scenario's because
 * its hub's escrow is on the earlier chain alone, and nothing sets it up
 * again on the new one, so its members could neither deposit nor settle
 * (spec/areas/hubs.md#founding-a-hub).
 */
export function startsFresh(def: ScenarioDef): string | undefined {
  if (live(def)) {
    return "runs on a new fake mainnet, which never resets";
  }
  if (def.hub !== undefined || def.hubs !== undefined) {
    return "founds its hub on a new fake ledger, since its escrow stays on the old one";
  }
  return undefined;
}

/** Whether `def`'s nodes resume with their data after a restart: unless
 * they start from scratch ({@linkcode startsFresh}). */
export function resumes(def: ScenarioDef): boolean {
  return startsFresh(def) === undefined;
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
 * What is wrong with `def`, if anything: with its periphery nodes, each of
 * which is one of its nodes, once, and no bootstrap node, since a node list
 * is served only by core nodes, and is pinned to at least one core node of
 * it, each once (the node checks how many it may name, `DASSIE_UPLINKS`);
 * or with its hub ({@linkcode invalidHub}).
 */
export function invalid(def: ScenarioDef): string | undefined {
  return invalidPeriphery(def) ?? invalidHub(def);
}

function invalidPeriphery(def: ScenarioDef): string | undefined {
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

/**
 * What is wrong with `def`'s hub, if anything: as many validators as a hub
 * is founded with, each one of its nodes, once; a sponsor among its nodes
 * that runs the owner book of XRP, which funds the escrow; and members,
 * each one of its nodes, once, that run the hub's scheme and the owner
 * book alone, as nodes added later do, the book peering with nobody: a
 * peering two members make before their hub accounts exist stays direct
 * (spec/areas/hubs.md#in-the-node). No other node runs the hub's scheme,
 * whose config is the founded hub's rules, never the scenario's.
 */
export function invalidHub(def: ScenarioDef): string | undefined {
  const hub = def.hub;
  if (hub === undefined) return undefined;
  const count = def.schemes.length;
  const inRange = (index: number) =>
    Number.isSafeInteger(index) && index >= 0 && index < count;
  const { validators, sponsor, members } = hub;
  if (
    validators.length < MIN_HUB_VALIDATORS ||
    validators.length > MAX_HUB_VALIDATORS
  ) {
    return `a hub has ${MIN_HUB_VALIDATORS} to ${MAX_HUB_VALIDATORS} validators, not ${validators.length}`;
  }
  const roles = [["validator", validators], ["member", members]] as const;
  for (const [role, nodes] of roles) {
    if (new Set(nodes).size !== nodes.length) return `a hub ${role} twice`;
    const stray = nodes.find((index) => !inRange(index));
    if (stray !== undefined) return `no node ${stray}`;
  }
  if (!inRange(sponsor)) return `no node ${sponsor}`;
  if (!def.schemes[sponsor].includes(HUB_OWNER_BOOK)) {
    return `the sponsor runs no ${HUB_OWNER_BOOK}, which funds the escrow`;
  }
  if (members.length === 0) return "a hub has no member";
  const joining = [HUB_OWNER_BOOK, HUB_SCHEME].sort();
  const joins = (schemes: readonly string[]) =>
    schemes.length === joining.length &&
    [...schemes].sort().every((scheme, i) => scheme === joining[i]);
  for (let index = 0; index < count; index++) {
    const schemes = def.schemes[index];
    const member = members.includes(index);
    if (member && !joins(schemes)) {
      return `hub member ${index} runs ${schemes.join(", ")}, not ${
        joining.join(" and ")
      } alone`;
    }
    if (!member && schemes.includes(HUB_SCHEME)) {
      return `node ${index} runs ${HUB_SCHEME} and is no hub member`;
    }
  }
  if (!joins(def.addedSchemes)) {
    return `added nodes run ${def.addedSchemes.join(", ")}, not ${
      joining.join(" and ")
    } alone`;
  }
  if (def.configs?.[HUB_SCHEME] !== undefined) {
    return `${HUB_SCHEME}'s config is the founded hub's rules`;
  }
  if (!ownerBookOnly(def.configs?.[HUB_OWNER_BOOK])) {
    return `${HUB_OWNER_BOOK} peers: its config must say "peers": false`;
  }
  return undefined;
}

/** Whether an XRP Ledger scheme's `config` makes it an owner book, which
 * peers with nobody (spec/areas/xrpl.md#owner-books). */
function ownerBookOnly(config: string | undefined): boolean {
  if (config === undefined) return false;
  try {
    return (JSON.parse(config) as { peers?: unknown }).peers === false;
  } catch {
    return false;
  }
}

/** Why `def` cannot run, with the fake XRP Ledger or without it. */
export function unavailable(
  def: ScenarioDef,
  fakeLedger: boolean,
): string | undefined {
  if (def.unavailable) return def.unavailable;
  if (live(def) && !fakeLedger) {
    return "needs --xrpl fake: the live realm runs only against the fake ledger's mainnet, never with real money";
  }
  if (def.mint !== undefined && !fakeLedger) {
    return `needs --xrpl fake: only the fake ledger's issuer mints ${def.mint.token} on demand`;
  }
  if (def.pool !== undefined && !fakeLedger) {
    return `needs --xrpl fake: only the fake ledger's issuer makes a pool of ${def.pool.token}`;
  }
  if (def.hub !== undefined && !fakeLedger) {
    return "needs --xrpl fake: each run founds a hub of its own, whose escrow stays off the public testnet";
  }
  return undefined;
}
