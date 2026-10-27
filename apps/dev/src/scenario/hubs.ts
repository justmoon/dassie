/**
 * The settlement hubs of a scenario (spec/areas/dev.md#scenarios): its one
 * hub of XRP (`ScenarioDef.hub`), or several (`ScenarioDef.hubs`), each of
 * XRP or of a dollar token, one per asset, on the fake ledger's network of
 * the scenario's realm: its testnet, or its mainnet in a live scenario.
 * Each is founded on the fake ledger by its sponsor, whose owner book of
 * XRP funds its escrow, and joined by its members with the scheme its
 * network and asset name (spec/decisions/hub-scheme-names.md), funded from
 * their owner book of that asset. A token hub's members run the owner book
 * of XRP too: before a withdrawal, a member tops the escrow's XRP up from
 * it (spec/decisions/token-hubs.md).
 *
 * @module
 */

import {
  type HubDef,
  type IssuedToken,
  ledgerNetwork,
  MAX_HUB_VALIDATORS,
  MIN_HUB_VALIDATORS,
  type ScenarioDef,
  TOKEN_SCHEMES,
} from "./scenarios.ts";
import type { XrplNetwork } from "../fake_ledger.ts";

/**
 * A hub of a scenario: of XRP, or of `token`, on the fake ledger's
 * `network`, its testnet unless it says mainnet ({@linkcode hubsOf} says
 * which for a scenario's hubs).
 */
export type ScenarioHub = HubDef & {
  readonly token?: IssuedToken;
  readonly network?: XrplNetwork;
};

/** A token as a hub's terms name it, as the XRP Ledger writes it: its
 * currency code in 40 hex digits and its issuer's address. */
export interface TokenAsset {
  readonly currency: string;
  readonly issuer: string;
}

/** What a hub of one asset on one network is: the scheme its members join
 * it with, the ledger that scheme books on, and the owner book of the
 * asset, which funds it. */
interface HubKind {
  readonly scheme: string;
  readonly ledger: string;
  readonly book: string;
}

/**
 * The hub of XRP on each network of the fake ledger. Its owner book of XRP
 * funds the escrow of every hub a sponsor founds there, and a member's
 * account in it (spec/areas/hubs.md#in-the-node).
 */
export const XRP_HUBS: Readonly<Record<XrplNetwork, HubKind>> = {
  testnet: {
    scheme: "hub-xrpl-testnet-xrp",
    ledger: "hub-xrpl-testnet-xrp+xrp",
    book: "xrpl-testnet-xrp",
  },
  mainnet: {
    scheme: "hub-xrpl-xrp",
    ledger: "hub-xrpl-xrp+xrp",
    book: "xrpl-xrp",
  },
};

/**
 * Each token a hub on the fake ledger may hold: its network, its scheme,
 * its ledger and its asset. The fake ledger's issuers are the networks'
 * own: DSUSD's on its testnet (`dassie_xrpl::dsusd::ISSUER`), RLUSD's on
 * its mainnet (`dassie_xrpl::rlusd::ISSUER`).
 */
export const TOKEN_HUBS: Readonly<
  Record<
    IssuedToken,
    {
      network: XrplNetwork;
      scheme: string;
      ledger: string;
      asset: TokenAsset;
    }
  >
> = {
  DSUSD: {
    network: "testnet",
    scheme: "hub-xrpl-testnet-dsusd",
    ledger: "hub-xrpl-testnet-dsusd+usd",
    asset: {
      currency: "4453555344000000000000000000000000000000",
      issuer: "rURgPoYNN7tGqBzGXFeCLmKYgpSjYgJP6M",
    },
  },
  RLUSD: {
    network: "mainnet",
    scheme: "hub-xrpl-rlusd",
    ledger: "hub-xrpl-rlusd+usd",
    asset: {
      currency: "524C555344000000000000000000000000000000",
      issuer: "rMxCKbEDwqr76QuheSUMdEGf4B9xJ8m5De",
    },
  },
};

/** The hubs of each scenario, as {@linkcode hubsOf} gives them: the same
 * ones every time, so that they compare as themselves. */
const HUBS = new WeakMap<ScenarioDef, readonly ScenarioHub[]>();

/**
 * The hubs of `def`: its `hubs`, or its one `hub`, or none, each on the
 * fake ledger's network of `def`'s realm.
 */
export function hubsOf(def: ScenarioDef): readonly ScenarioHub[] {
  const known = HUBS.get(def);
  if (known !== undefined) return known;
  const network = ledgerNetwork(def);
  const hubs = (def.hubs ?? (def.hub === undefined ? [] : [def.hub])).map(
    (hub): ScenarioHub => ({ ...hub, network }),
  );
  HUBS.set(def, hubs);
  return hubs;
}

/** The owner book of XRP of `def`'s nodes, on the fake ledger's network of
 * its realm: what funds the escrow of a hub a sponsor founds. */
export function xrpOwnerBook(def: ScenarioDef): string {
  return XRP_HUBS[ledgerNetwork(def)].book;
}

/** What a token hub of `hub`'s token is, if it is one. */
function tokenHub(hub: ScenarioHub) {
  return hub.token === undefined ? undefined : TOKEN_HUBS[hub.token];
}

/** The hub of XRP on `hub`'s network. */
function xrpHub(hub: ScenarioHub): HubKind {
  return XRP_HUBS[hub.network ?? "testnet"];
}

/** The scheme `hub`'s members join it with. */
export function hubScheme(hub: ScenarioHub): string {
  return tokenHub(hub)?.scheme ?? xrpHub(hub).scheme;
}

/** The ledger `hub`'s scheme books on. */
export function hubLedger(hub: ScenarioHub): string {
  return tokenHub(hub)?.ledger ?? xrpHub(hub).ledger;
}

/** The owner book of `hub`'s asset, from which its members fund it. */
export function hubOwnerBook(hub: ScenarioHub): string {
  return hub.token === undefined ? xrpHub(hub).book : TOKEN_SCHEMES[hub.token];
}

/** The owner book's ledger: the scheme's, in its asset. */
export function hubOwnerLedger(hub: ScenarioHub): string {
  return `${hubOwnerBook(hub)}+${hub.token === undefined ? "xrp" : "usd"}`;
}

/**
 * The terms `hub` is founded with, as `FoundHubRequest.terms` takes them:
 * the starting ones for a hub of XRP, and for a token hub the token, with
 * the starting fees of a token hub (spec/areas/hubs.md#founding-a-hub).
 */
export function hubTerms(hub: ScenarioHub): string {
  const token = tokenHub(hub);
  return token === undefined ? "" : JSON.stringify({ asset: token.asset });
}

/** The schemes of every hub of `def`. */
export function hubSchemes(def: ScenarioDef): string[] {
  return hubsOf(def).map(hubScheme);
}

/** Whether node `index` of `def` validates any of its hubs. */
export function validatesAny(def: ScenarioDef, index: number): boolean {
  return hubsOf(def).some((hub) => hub.validators.includes(index));
}

/**
 * Whether node `index` of `def` is a member of `hub`: one it names, or one
 * added later whose schemes include the hub's.
 */
export function joins(
  def: ScenarioDef,
  hub: ScenarioHub,
  index: number,
): boolean {
  if (hub.members.includes(index)) return true;
  return index >= def.schemes.length &&
    def.addedSchemes.includes(hubScheme(hub));
}

/** The nodes that sponsor `def`'s hubs, by index, each once. */
export function sponsors(def: ScenarioDef): number[] {
  return [...new Set(hubsOf(def).map((hub) => hub.sponsor))];
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

/**
 * What is wrong with `def`'s `hubs`, if anything; its one `hub` is
 * `invalidHub`'s to check. Each hub has as many validators as a hub is
 * founded with, each one of its nodes, once; a sponsor among its nodes
 * that runs the owner book of XRP of the scenario's realm, which funds the
 * escrow; and members, each one of its nodes, once. No two hubs hold one
 * asset, and a token hub's token is one of the fake ledger's network of
 * the scenario's realm, the one it mints the scenario's nodes and prices
 * in its pool. A member runs its hubs' schemes and the owner
 * books they fund from, the book of XRP among them, and nothing else, so
 * it peers over hubs alone; no other node runs a hub's scheme, whose
 * config is the founded hub's rules, never the scenario's; nodes added
 * later run what a member does; and every owner book a member runs peers
 * with nobody.
 */
export function invalidHubs(def: ScenarioDef): string | undefined {
  if (def.hubs === undefined) return undefined;
  if (def.hub !== undefined) return "a scenario has a hub or hubs, not both";
  const hubs = hubsOf(def);
  if (hubs.length === 0) return "no hub in hubs";
  const count = def.schemes.length;
  const inRange = (index: number) =>
    Number.isSafeInteger(index) && index >= 0 && index < count;
  const network = ledgerNetwork(def);
  const xrpBook = xrpOwnerBook(def);
  const assets = new Set<string>();
  for (const hub of hubs) {
    const asset = hub.token ?? "XRP";
    if (hub.token !== undefined && TOKEN_HUBS[hub.token].network !== network) {
      return `no hub of ${hub.token} on the fake ${network}`;
    }
    if (assets.has(asset)) return `two hubs of ${asset}`;
    assets.add(asset);
    if (hub.token !== undefined) {
      if (def.mint?.token !== hub.token) {
        return `the ${hub.token} hub's members are minted no ${hub.token}`;
      }
      if (def.pool?.token !== hub.token) {
        return `no pool prices ${hub.token}, whose guard halts its hub's ledger`;
      }
    }
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
    if (!def.schemes[sponsor].includes(xrpBook)) {
      return `the sponsor runs no ${xrpBook}, which funds the escrow`;
    }
    if (members.length === 0) return "a hub has no member";
  }
  const schemes = hubSchemes(def);
  // What a node runs that joins these hubs: their schemes and their owner
  // books, the book of XRP always.
  const joining = (these: readonly ScenarioHub[]) =>
    [
      ...new Set([
        xrpBook,
        ...these.flatMap((hub) => [hubOwnerBook(hub), hubScheme(hub)]),
      ]),
    ].sort();
  const same = (a: readonly string[], b: readonly string[]) =>
    a.length === b.length && [...a].sort().every((s, i) => s === b[i]);
  for (let index = 0; index < count; index++) {
    const runs = def.schemes[index];
    const of = hubs.filter((hub) => hub.members.includes(index));
    if (of.length > 0 && !same(runs, joining(of))) {
      return `hub member ${index} runs ${runs.join(", ")}, not ${
        joining(of).join(", ")
      } alone`;
    }
    const stray = runs.find((scheme) =>
      schemes.includes(scheme) && !of.some((hub) => hubScheme(hub) === scheme)
    );
    if (stray !== undefined) {
      return `node ${index} runs ${stray} and is no member of its hub`;
    }
  }
  const added = hubs.filter((hub) => joins(def, hub, count));
  if (added.length === 0 || !same(def.addedSchemes, joining(added))) {
    return `added nodes run ${
      def.addedSchemes.join(", ")
    }, not a member's schemes alone`;
  }
  for (const scheme of schemes) {
    if (def.configs?.[scheme] !== undefined) {
      return `${scheme}'s config is the founded hub's rules`;
    }
  }
  const books = new Set([xrpBook, ...hubs.map(hubOwnerBook)]);
  for (const book of books) {
    if (!ownerBookOnly(def.configs?.[book])) {
      return `${book} peers: its config must say "peers": false`;
    }
  }
  return undefined;
}
