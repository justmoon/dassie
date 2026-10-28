/**
 * What the end-to-end tests of mainnet's schemes in the live realm share
 * (`mainnet_test.ts`), and of the scenarios' settlement hubs
 * (`cross_currency_test.ts`, `live_hub_test.ts`, `two_hubs_test.ts`,
 * `xrpl_deposit_test.ts`,
 * and the market maker's and peer swaps' over hubs of XRP and DSUSD): a
 * node's ledger, whether it runs in the live realm, how the hubs' members
 * join them, when settling over them is over, and a payment from a BTP
 * client of one node to another's receiver.
 *
 * Settling is over when the nodes say so, never after a span of time: no
 * settlement message on its way to a peer, and no settling under way over
 * any hub (`HubStatus.settling`), with every book in step with its account
 * (spec/areas/testing.md#end-to-end).
 */

import { assert, assertEquals } from "@std/assert";
import { Buffer } from "node:buffer";
import { readFile } from "node:fs/promises";
import WebSocket from "#ws";
import { createConnection } from "ilp-protocol-stream";
import { LedgerConnection, Realm } from "@dassie/rpc/proto";

import { BtpClient } from "./btp.ts";
import {
  HUB_LEDGER,
  HUB_OWNER_BOOK,
  HUB_SCHEME,
  hubLedger,
  hubOwnerBook,
  hubOwnerLedger,
  hubScheme,
  hubsOf,
  joins,
  type Scenario,
  type ScenarioHub,
  type ScenarioNode,
  xrpOwnerBook,
} from "@dassie/dev/scenario";

/** Node `node`'s ledger `id`, as its owner sees it. */
export async function ledger(node: ScenarioNode, id: string) {
  const { ledgers } = await node.rpc.call("listLedgers", {});
  const found = ledgers.find((l) => l.id === id);
  assert(found, `${node.name} has no ${id}`);
  return found;
}

/**
 * Every node runs in the live realm: its ledgers are of it, the schemes it
 * offers are mainnet's alone, and an incoming payment's address is under
 * `g.`.
 */
export async function assertLive(
  node: ScenarioNode,
  id: string,
): Promise<void> {
  assertEquals((await ledger(node, id)).realm, Realm.LIVE);
  const { schemes } = await node.rpc.call("listSettlementSchemes", {});
  assert(schemes.length > 0, `${node.name} offers no scheme`);
  for (const scheme of schemes) {
    assertEquals(scheme.realm, Realm.LIVE, `${node.name} offers ${scheme.id}`);
  }
  assertEquals(
    schemes.map((scheme) => scheme.id).sort(),
    ["xrpl-rlusd", "xrpl-xrp"],
  );
  const { payment } = await node.rpc.call("createIncomingPayment", {});
  assert(
    payment?.destinationAccount.startsWith("g."),
    payment?.destinationAccount,
  );
}

/**
 * A client of `from` with a BTP token on ledger `id` pays `amount` to an
 * incoming payment of `to`'s on ledger `receive`, its default ledger if
 * empty, over STREAM; the ILP address it paid to.
 */
export async function pay(
  scenario: Scenario,
  [from, to]: [ScenarioNode, ScenarioNode],
  id: string,
  amount: bigint,
  receive = "",
): Promise<string> {
  const token = await from.rpc.call("createBtpToken", {
    label: "e2e",
    ledger: id,
  });
  const { payment: credentials } = await to.rpc.call(
    "createIncomingPayment",
    { ledger: receive },
  );
  assert(credentials);
  const server = token.url.replace(`@${from.hostname}:`, "@localhost:");
  const ca = await readFile(scenario.caFile, "utf8");
  const plugin = new BtpClient({ server }, {
    WebSocket: class extends WebSocket {
      constructor(url: string) {
        super(url, { ca });
      }
    },
  });
  await plugin.connect();
  try {
    const connection = await createConnection({
      plugin,
      destinationAccount: credentials.destinationAccount,
      sharedSecret: Buffer.from(credentials.sharedSecret),
    });
    const stream = connection.createStream();
    await stream.sendTotal(Number(amount), { timeout: 240_000 });
    assertEquals(BigInt(stream.totalSent), amount);
    await connection.end();
  } finally {
    await plugin.disconnect();
  }
  return credentials.destinationAccount;
}

/** The `hub` scenario's members' owner book of XRP, from which they fund
 * the hub. */
export const HUB_OWNER_LEDGER = `${HUB_OWNER_BOOK}+xrp`;
/**
 * How long the hub's members may take to open their accounts, peer over
 * it, or settle: each move and operation waits for a validated ledger and
 * a final block, and a member reads the hub again after a restart. A
 * hang's guard, not what decides a test.
 */
export const HUB_WITHIN_MS = 180_000;
/** Internal units in a drop: XRP's ledgers are kept at scale 9, a drop at
 * scale 6. */
const UNITS_PER_DROP = 1_000n;

/** Whether `node` peers with `peer` over the scenario's one hub. */
export async function peersOverHub(
  node: ScenarioNode,
  peer: ScenarioNode,
): Promise<boolean> {
  const { peers } = await node.rpc.call("listPeers", {});
  return peers.some((p) =>
    p.node === peer.id && p.state === "peered" &&
    p.methods.some((m) => m.scheme === HUB_SCHEME)
  );
}

/** Whether `node`'s owner book of XRP follows the ledger and holds funds,
 * as the faucet funds it. */
export async function fundedInOwnerBook(node: ScenarioNode): Promise<boolean> {
  const view = await ledger(node, HUB_OWNER_LEDGER);
  return view.connection === LedgerConnection.SYNCED && view.ownerBalance > 0n;
}

/** Whether `node` follows the scenario's one hub and has an account in it
 * with funds. */
export async function fundedInHub(node: ScenarioNode): Promise<boolean> {
  const view = await ledger(node, HUB_LEDGER);
  return view.connection === LedgerConnection.SYNCED && view.ownerBalance > 0n;
}

// Over the scenario's hubs, one or several: each hub's members, whether a
// node is funded in a hub, what its accounts in its hubs hold, and when
// settling over all of them is over.

/** The members of `hub`, one of `scenario`'s hubs, among its running
 * nodes: those it names, and nodes added later that join it. */
export function hubMembers(
  scenario: Scenario,
  hub: ScenarioHub,
): ScenarioNode[] {
  return scenario.nodes.filter((node) => joins(scenario.def, hub, node.index));
}

/** The hubs of `scenario` that `node` is a member of. */
export function hubsOfNode(
  scenario: Scenario,
  node: ScenarioNode,
): ScenarioHub[] {
  return hubsOf(scenario.def).filter((hub) =>
    joins(scenario.def, hub, node.index)
  );
}

/** The hub of `scenario` whose scheme is `scheme`. */
export function hubOf(scenario: Scenario, scheme: string): ScenarioHub {
  const hub = hubsOf(scenario.def).find((hub) => hubScheme(hub) === scheme);
  assert(hub, `the scenario has no hub of ${scheme}`);
  return hub;
}

/** Whether `node` follows `hub` and has an account in it with funds. */
export async function fundedInEach(
  node: ScenarioNode,
  hubs: readonly ScenarioHub[],
): Promise<boolean> {
  for (const hub of hubs) {
    const view = await ledger(node, hubLedger(hub));
    if (view.connection !== LedgerConnection.SYNCED) return false;
    if (view.ownerBalance <= 0n) return false;
  }
  return true;
}

/** `node`'s account in `hub`, as its owner sees it. */
export async function hubStatus(node: ScenarioNode, hub: ScenarioHub) {
  const { hubs } = await node.rpc.call("getHubStatus", {});
  const status = hubs.find((status) => status.scheme === hubScheme(hub));
  assert(status, `${node.name} is in no hub of ${hubScheme(hub)}`);
  return status;
}

/**
 * Whether `node` says nothing is on its way: no settlement message to a
 * peer unacknowledged, and no settling under way over any of its hubs
 * (`HubStatus.settling`), which covers a move between a hub and its owner
 * book, on its way or due, a check or top-up of a token hub's escrow, an
 * operation on its way to the hub, a packet not yet settled by its
 * promises, a settlement of its own not yet final, and a peer's request to
 * settle it has yet to fund.
 */
async function nothingOnItsWay(node: ScenarioNode): Promise<boolean> {
  const { channels } = await node.rpc.call("listSettlementChannels", {});
  if (channels.some((channel) => channel.sending)) return false;
  const { hubs } = await node.rpc.call("getHubStatus", {});
  return hubs.every((hub) => hub.moving === undefined && !hub.settling);
}

/**
 * What the external asset that `node`'s scheme `scheme` settles through
 * holds of `code`, or of its one asset if `code` is empty, less what the
 * ledger holds back, in the units of the scheme's ledger, at scale 9.
 */
async function heldIn(
  node: ScenarioNode,
  scheme: string,
  code: string,
): Promise<bigint> {
  const { externalAssets } = await node.rpc.call("listLedgers", {});
  const asset = externalAssets.find((asset) =>
    asset.settlementSchemes.includes(scheme)
  );
  assert(asset, `${node.name} has no account for ${scheme}`);
  const holding = code === ""
    ? (asset.holdings.length === 1 ? asset.holdings[0] : undefined)
    : asset.holdings.find((holding) => holding.assetCode === code);
  assert(holding, `${node.name} shows no ${code || "one asset"} for ${scheme}`);
  // The books count whole drops of XRP or millionths of a token: a line
  // shows a token at a finer scale.
  const held = holding.amount - holding.reserve;
  const scale = BigInt(holding.assetScale);
  const micros = scale >= 6n
    ? held / 10n ** (scale - 6n)
    : held * 10n ** (6n - scale);
  return micros * UNITS_PER_DROP;
}

/**
 * Whether `node` of `scenario` is quiet over all its hubs and its books
 * add up, each by its own books alone
 * (spec/areas/testing.md#each-node-judged-by-its-own-books): nothing on
 * its way, as the node says; every ledger synced, with nothing the owner
 * has pending; each owner book holding what its XRP Ledger account holds
 * of its asset; and each hub's book what its account in the hub holds and
 * its peers' holds toward it, as the hub's log shows them to the node,
 * since it booked their settlements as they funded them.
 */
async function inStepOverHubs(
  scenario: Scenario,
  node: ScenarioNode,
): Promise<boolean> {
  if (!(await nothingOnItsWay(node))) return false;
  const xrpBook = xrpOwnerBook(scenario.def);
  const expected: [string, () => Promise<bigint>][] = [
    [`${xrpBook}+xrp`, () => heldIn(node, xrpBook, "XRP")],
  ];
  for (const hub of hubsOfNode(scenario, node)) {
    const book = hubOwnerBook(hub);
    if (book !== xrpBook) {
      const code = hub.token ?? "XRP";
      expected.push([hubOwnerLedger(hub), () => heldIn(node, book, code)]);
    }
    expected.push([hubLedger(hub), async () => {
      const { taking } = await hubStatus(node, hub);
      const holds = taking.reduce((sum, hold) => sum + hold.amount, 0n);
      return await heldIn(node, hubScheme(hub), "") + holds;
    }]);
  }
  for (const [id, held] of expected) {
    const view = await ledger(node, id);
    if (view.connection !== LedgerConnection.SYNCED) return false;
    if (view.ownerPending !== 0n) return false;
    if (view.onLedgerBalance !== await held()) return false;
  }
  return true;
}

/** What a member's books hold of each of its hubs and owner books, and
 * what its owner has on each, by ledger. */
export async function booksOverHubs(
  scenario: Scenario,
  node: ScenarioNode,
): Promise<Record<string, [bigint, bigint]>> {
  const ids = new Set([`${xrpOwnerBook(scenario.def)}+xrp`]);
  for (const hub of hubsOfNode(scenario, node)) {
    ids.add(hubOwnerLedger(hub));
    ids.add(hubLedger(hub));
  }
  const books: Record<string, [bigint, bigint]> = {};
  for (const id of [...ids].sort()) {
    const view = await ledger(node, id);
    books[id] = [view.onLedgerBalance, view.ownerBalance];
  }
  return books;
}

/** JSON of bigints, as text. */
function text(_key: string, value: unknown) {
  return typeof value === "bigint" ? `${value}` : value;
}

/**
 * Settling over every hub of `scenario` is over: every member in step,
 * with nothing on its way, as each says; and so again once all were read,
 * with the same books, so that nothing one member started while the
 * others were read slips by.
 */
export async function settledOverHubs(scenario: Scenario): Promise<boolean> {
  const nodes = scenario.nodes.filter((node) =>
    hubsOfNode(scenario, node).length > 0
  );
  const books = async () =>
    JSON.stringify(
      await Promise.all(nodes.map((node) => booksOverHubs(scenario, node))),
      text,
    );
  for (const node of nodes) {
    if (!(await inStepOverHubs(scenario, node))) return false;
  }
  const first = await books();
  for (const node of nodes) {
    if (!(await inStepOverHubs(scenario, node))) return false;
  }
  return await books() === first;
}

/** Whether `node` peers with `peer`, over any method. */
export async function peersWith(
  node: ScenarioNode,
  peer: ScenarioNode,
): Promise<boolean> {
  const { peers } = await node.rpc.call("listPeers", {});
  return peers.some((p) => p.node === peer.id && p.state === "peered");
}

/**
 * The ledgers on which `node` keeps a peer account for `peer`: one for
 * each method of their peering, a hub's ledger each.
 */
export async function peerLedgers(
  node: ScenarioNode,
  peer: ScenarioNode,
): Promise<string[]> {
  const { accounts } = await node.rpc.call("listAccounts", {});
  const suffix = `:assets/interledger/${peer.id}`;
  const ids = accounts
    .filter((account) => account.path.endsWith(suffix))
    .map((account) => account.path.slice(0, -suffix.length));
  return [...new Set(ids)].sort();
}

/**
 * Waits until the members of `scenario`'s hubs, which the scenario founded
 * as it started, joined them: each opened its account in each of its hubs
 * by a deposit from its owner book of the hub's asset, by itself
 * (spec/areas/hubs.md#in-the-node); every two members of a hub peer, with
 * a peer account on the ledger of each hub both are in; no member peers
 * but over a hub; each holds funds toward the other in each of those hubs,
 * so that either can pay the other over it; and settling is over.
 *
 * A member may peer before its own deposit opened its account, with a
 * peer that has one. That peer learns of the account by asking the hub,
 * at most a minute later (`LOOK_UP_EVERY_MS` in dassie-hub's member), and
 * until then neither asks the other to fund a hold: the member could send
 * nothing over the hub, and a payment from it would fail.
 */
export async function joinedHubs(scenario: Scenario): Promise<void> {
  const hubs = hubsOf(scenario.def);
  const ledgersOf = (node: ScenarioNode) =>
    hubsOfNode(scenario, node).map(hubLedger).sort();
  const members = scenario.nodes.filter((node) => ledgersOf(node).length > 0);
  await scenario.until("members funded in their hubs", async () => {
    for (const node of members) {
      if (!(await fundedInEach(node, hubsOfNode(scenario, node)))) {
        return false;
      }
    }
    return true;
  }, HUB_WITHIN_MS);
  const pairs: [ScenarioNode, ScenarioNode, string[]][] = [];
  for (const node of members) {
    for (const peer of members) {
      const shared = ledgersOf(node).filter((id) =>
        ledgersOf(peer).includes(id)
      );
      if (peer !== node && shared.length > 0) pairs.push([node, peer, shared]);
    }
  }
  await scenario.until("peered over every hub both are in", async () => {
    for (const [node, peer, shared] of pairs) {
      if (!(await peersWith(node, peer))) return false;
      const on = await peerLedgers(node, peer);
      if (on.join() !== shared.join()) return false;
    }
    return true;
  }, HUB_WITHIN_MS);
  const schemes = hubs.map(hubScheme);
  for (const node of members) {
    const { peers } = await node.rpc.call("listPeers", {});
    for (const { methods } of peers) {
      for (const { scheme } of methods) {
        assert(schemes.includes(scheme), `${node.name}: ${scheme}`);
      }
    }
  }
  await scenario.until("holds toward every peer over each hub", async () => {
    for (const [node, peer] of pairs) {
      for (const hub of hubsOfNode(scenario, node)) {
        if (!hubsOfNode(scenario, peer).includes(hub)) continue;
        const { paying } = await hubStatus(node, hub);
        const holds = paying.some((hold) =>
          hold.peer === peer.id && hold.amount > 0n
        );
        if (!holds) return false;
      }
    }
    return true;
  }, HUB_WITHIN_MS);
  await scenario.until(
    "in step over every hub",
    () => settledOverHubs(scenario),
    HUB_WITHIN_MS,
  );
}
