/**
 * The settlement hubs of a scenario (spec/areas/dev.md#scenarios): its one
 * hub of XRP (`ScenarioDef.hub`), or several (`ScenarioDef.hubs`), each of
 * XRP or of a dollar token, one per asset. Each is founded on the fake
 * ledger by its sponsor, whose owner book of XRP funds its escrow, and
 * joined by its members with the scheme its asset names
 * (spec/decisions/hub-scheme-names.md), funded from their owner book of
 * that asset. A token hub's members run the owner book of XRP too: before
 * a withdrawal, a member tops the escrow's XRP up from it
 * (spec/decisions/token-hubs.md).
 *
 * @module
 */

import {
  HUB_LEDGER,
  HUB_OWNER_BOOK,
  HUB_SCHEME,
  type HubDef,
  type IssuedToken,
  MAX_HUB_VALIDATORS,
  MIN_HUB_VALIDATORS,
  type ScenarioDef,
  TOKEN_SCHEMES,
} from "./scenarios.ts";

/** A hub of a scenario: of XRP, or of `token`. */
export type ScenarioHub = HubDef & { readonly token?: IssuedToken };

/** A token as a hub's terms name it, as the XRP Ledger writes it: its
 * currency code in 40 hex digits and its issuer's address. */
export interface TokenAsset {
  readonly currency: string;
  readonly issuer: string;
}

/**
 * Each token a hub on the fake ledger may hold: its scheme, its ledger and
 * its asset. The fake ledger's DSUSD issuer is the testnet's own
 * (`dassie_xrpl::dsusd::ISSUER`). RLUSD's hub is on mainnet, which no hub
 * scenario runs.
 */
export const TOKEN_HUBS: Readonly<
  Partial<
    Record<
      IssuedToken,
      { scheme: string; ledger: string; asset: TokenAsset }
    >
  >
> = {
  DSUSD: {
    scheme: "hub-xrpl-testnet-dsusd",
    ledger: "hub-xrpl-testnet-dsusd+usd",
    asset: {
      currency: "4453555344000000000000000000000000000000",
      issuer: "rURgPoYNN7tGqBzGXFeCLmKYgpSjYgJP6M",
    },
  },
};

/** The hubs of `def`: its `hubs`, or its one `hub`, or none. */
export function hubsOf(def: ScenarioDef): readonly ScenarioHub[] {
  if (def.hubs !== undefined) return def.hubs;
  return def.hub === undefined ? [] : [def.hub];
}

/** What a token hub of `hub`'s token is, if it is one. */
function tokenHub(hub: ScenarioHub) {
  if (hub.token === undefined) return undefined;
  const found = TOKEN_HUBS[hub.token];
  if (found === undefined) throw new Error(`no hub of ${hub.token}`);
  return found;
}

/** The scheme `hub`'s members join it with. */
export function hubScheme(hub: ScenarioHub): string {
  return tokenHub(hub)?.scheme ?? HUB_SCHEME;
}

/** The ledger `hub`'s scheme books on. */
export function hubLedger(hub: ScenarioHub): string {
  return tokenHub(hub)?.ledger ?? HUB_LEDGER;
}

/** The owner book of `hub`'s asset, from which its members fund it. */
export function hubOwnerBook(hub: ScenarioHub): string {
  return hub.token === undefined ? HUB_OWNER_BOOK : TOKEN_SCHEMES[hub.token];
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
 * that runs the owner book of XRP, which funds the escrow; and members,
 * each one of its nodes, once. No two hubs hold one asset, and a token
 * hub's token is the one the fake ledger mints the scenario's nodes and
 * prices in its pool. A member runs its hubs' schemes and the owner
 * books they fund from, the book of XRP among them, and nothing else, so
 * it peers over hubs alone; no other node runs a hub's scheme, whose
 * config is the founded hub's rules, never the scenario's; nodes added
 * later run what a member does; and every owner book a member runs peers
 * with nobody.
 */
export function invalidHubs(def: ScenarioDef): string | undefined {
  const hubs = def.hubs;
  if (hubs === undefined) return undefined;
  if (def.hub !== undefined) return "a scenario has a hub or hubs, not both";
  if (hubs.length === 0) return "no hub in hubs";
  const count = def.schemes.length;
  const inRange = (index: number) =>
    Number.isSafeInteger(index) && index >= 0 && index < count;
  const assets = new Set<string>();
  for (const hub of hubs) {
    const asset = hub.token ?? "XRP";
    if (hub.token !== undefined && TOKEN_HUBS[hub.token] === undefined) {
      return `no hub of ${hub.token} on the fake testnet`;
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
    if (!def.schemes[sponsor].includes(HUB_OWNER_BOOK)) {
      return `the sponsor runs no ${HUB_OWNER_BOOK}, which funds the escrow`;
    }
    if (members.length === 0) return "a hub has no member";
  }
  const schemes = hubSchemes(def);
  // What a node runs that joins these hubs: their schemes and their owner
  // books, the book of XRP always.
  const joining = (these: readonly ScenarioHub[]) =>
    [
      ...new Set([
        HUB_OWNER_BOOK,
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
  const books = new Set([HUB_OWNER_BOOK, ...hubs.map(hubOwnerBook)]);
  for (const book of books) {
    if (!ownerBookOnly(def.configs?.[book])) {
      return `${book} peers: its config must say "peers": false`;
    }
  }
  return undefined;
}
