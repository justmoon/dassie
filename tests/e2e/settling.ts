/**
 * What the end-to-end tests of mainnet's schemes in the live realm share
 * (`mainnet_test.ts`, `mainnet_xrp_test.ts`), and of the `hub` scenario's
 * settlement hub (`hub_test.ts`, `xrpl_test.ts`, `xrpl_deposit_test.ts`):
 * a node's ledger, whether it runs in the live realm, what it owes its
 * peers by its own books, when settling is over, directly or over the hub,
 * how the hub's members join it, and a payment from a BTP client of one
 * node to another's receiver.
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
  type Scenario,
  type ScenarioNode,
} from "@dassie/dev/scenario";

/** Every node's trigger: one and a half of the largest packets
 * (spec/decisions/thresholds-in-packets.md). */
export const TRIGGER = 150_000_000n;
/** What a node extends to each peer. */
export const CREDIT = 1_100_000_000n;
/** Longer than a node's settlement interval with dev timings. */
const SETTLE_CHECK_MS = 5_000;

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
 * What each node owes each peer net on ledger `id`, by its own books, by
 * `node>peer`: its peer account less the credit it extends, negative when
 * the peer owes it.
 */
export async function peerAccounts(
  scenario: Scenario,
  id: string,
): Promise<Map<string, bigint>> {
  const byId = new Map(scenario.nodes.map((node) => [node.id, node.name]));
  const accounts = new Map<string, bigint>();
  for (const node of scenario.nodes) {
    const list = await node.rpc.call("listAccounts", {});
    for (const account of list.accounts) {
      for (const kind of ["assets/interledger", "contra/trust"]) {
        const prefix = `${id}:${kind}/`;
        if (!account.path.startsWith(prefix)) continue;
        const key = `${node.name}>${
          byId.get(account.path.slice(prefix.length))
        }`;
        const net = account.creditsPosted - account.debitsPosted;
        accounts.set(key, (accounts.get(key) ?? 0n) + net);
      }
    }
  }
  return accounts;
}

/**
 * Settling on ledger `id` went quiet: nothing on its way, every node synced
 * and, by `inStep`, in step with its account, and no node's books showing
 * a peer at its trigger.
 */
async function quiet(
  scenario: Scenario,
  id: string,
  inStep: (node: ScenarioNode) => Promise<boolean>,
): Promise<boolean> {
  for (const node of scenario.nodes) {
    const { channels } = await node.rpc.call("listSettlementChannels", {});
    if (channels.some((channel) => channel.sending)) return false;
    if ((await ledger(node, id)).connection !== LedgerConnection.SYNCED) {
      return false;
    }
    if (!(await inStep(node))) return false;
    const { peers } = await node.rpc.call("listPeers", {});
    if (peers.some((peer) => peer.creditOut + peer.owed <= TRIGGER)) {
      return false;
    }
  }
  return (await peerAccounts(scenario, id)).size > 0;
}

/**
 * Settling on ledger `id` is over: quiet, and still quiet with the same
 * books once a settlement interval has passed.
 */
export async function settled(
  scenario: Scenario,
  id: string,
  inStep: (node: ScenarioNode) => Promise<boolean> = () =>
    Promise.resolve(true),
): Promise<boolean> {
  if (!(await quiet(scenario, id, inStep))) return false;
  const views = () => Promise.all(scenario.nodes.map((n) => ledger(n, id)));
  const first = await views();
  await new Promise((resolve) => setTimeout(resolve, SETTLE_CHECK_MS));
  if (!(await quiet(scenario, id, inStep))) return false;
  const second = await views();
  return first.every((view, index) =>
    view.onLedgerBalance === second[index].onLedgerBalance &&
    view.ownerBalance === second[index].ownerBalance
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

/** The hub's members' owner book of XRP, from which they fund the hub. */
export const HUB_OWNER_LEDGER = `${HUB_OWNER_BOOK}+xrp`;
/**
 * How long the hub's members may take to open their accounts, peer over
 * it, or settle: each move and operation waits for a validated ledger and
 * a final block, and a member reads the hub again after a restart.
 */
export const HUB_WITHIN_MS = 180_000;
/** Internal units in a drop: XRP's ledgers are kept at scale 9, a drop at
 * scale 6. */
export const UNITS_PER_DROP = 1_000n;

/** The members of the scenario's hub. */
export function members(scenario: Scenario): ScenarioNode[] {
  const hub = scenario.def.hub;
  assert(hub, "the scenario has no hub");
  return scenario.nodes.filter((node) => hub.members.includes(node.index));
}

/**
 * What `node`'s account that its scheme `scheme` settles through holds of
 * XRP, as its ledger last said, less what the ledger holds back, in
 * internal units.
 */
async function held(node: ScenarioNode, scheme: string): Promise<bigint> {
  const { externalAssets } = await node.rpc.call("listLedgers", {});
  const asset = externalAssets.find((asset) =>
    asset.settlementSchemes.includes(scheme)
  );
  const holding = asset?.holdings.find((h) => h.assetCode === "XRP");
  assert(holding, `${node.name} shows no XRP held for ${scheme}`);
  assertEquals(holding.assetScale, 6);
  return (holding.amount - holding.reserve) * UNITS_PER_DROP;
}

/**
 * What `node` extends each of its peers over the hub, by peer ID: its hold
 * toward the peer, once none of its settlements to the peer is on its way
 * (spec/areas/hubs.md#in-the-node).
 */
async function holds(node: ScenarioNode): Promise<Map<string, bigint>> {
  const { peers } = await node.rpc.call("listPeers", {});
  return new Map(
    peers
      .filter((peer) => peer.scheme === HUB_SCHEME)
      .map((peer) => [peer.node, peer.creditOut]),
  );
}

/** What a member's books hold of the hub and of its owner book, and what
 * its owner has on each. */
export async function hubBooks(node: ScenarioNode) {
  const [hub, owner] = await Promise.all([
    ledger(node, HUB_LEDGER),
    ledger(node, HUB_OWNER_LEDGER),
  ]);
  return {
    hub: [hub.onLedgerBalance, hub.ownerBalance],
    owner: [owner.onLedgerBalance, owner.ownerBalance],
  };
}

/** What a member's owner has on both ledgers. */
export async function ownerFunds(node: ScenarioNode): Promise<bigint> {
  const { hub, owner } = await hubBooks(node);
  return hub[1] + owner[1];
}

/**
 * Whether member `node` of `scenario` is quiet and its books add up: both
 * ledgers synced, nothing on its way, no packet in flight; its book of XRP
 * holding what its XRP Ledger account holds, and its book of the hub what
 * its account in the hub holds and its peers' holds toward it, which it
 * booked as their settlements as they funded them, each less what its
 * ledger holds back (spec/areas/testing.md#each-node-judged-by-its-own-books).
 * A peer's hold toward it is read from the peer, as the credit the peer
 * extends it. That credit is the hold and the peer's settlements on their
 * way, so it is the hold only once the peer is quiet too: the check holds
 * once every member is, which {@linkcode settledOverHub} asks of each in
 * turn and again a settlement interval later.
 */
async function inStep(
  scenario: Scenario,
  node: ScenarioNode,
): Promise<boolean> {
  const { channels } = await node.rpc.call("listSettlementChannels", {});
  if (channels.some((channel) => channel.sending)) return false;
  const own = await holds(node);
  let theirs = 0n;
  for (const peer of members(scenario)) {
    if (own.has(peer.id)) theirs += (await holds(peer)).get(node.id) ?? 0n;
  }
  const expected = [
    [HUB_LEDGER, HUB_SCHEME, theirs],
    [HUB_OWNER_LEDGER, HUB_OWNER_BOOK, 0n],
  ] as const;
  for (const [id, scheme, peerHolds] of expected) {
    const view = await ledger(node, id);
    if (view.connection !== LedgerConnection.SYNCED) return false;
    if (view.ownerPending !== 0n) return false;
    if (view.onLedgerBalance !== await held(node, scheme) + peerHolds) {
      return false;
    }
  }
  return true;
}

/**
 * Settling over the hub is over: every member in step, and still in step
 * with the same books once a settlement interval has passed.
 */
export async function settledOverHub(scenario: Scenario): Promise<boolean> {
  const nodes = members(scenario);
  for (const node of nodes) if (!(await inStep(scenario, node))) return false;
  const first = JSON.stringify(await Promise.all(nodes.map(hubBooks)), text);
  await new Promise((resolve) => setTimeout(resolve, SETTLE_CHECK_MS));
  for (const node of nodes) if (!(await inStep(scenario, node))) return false;
  const second = JSON.stringify(await Promise.all(nodes.map(hubBooks)), text);
  return second === first;
}

/** JSON of bigints, as text. */
function text(_key: string, value: unknown) {
  return typeof value === "bigint" ? `${value}` : value;
}

/** Whether `node` peers with `peer` over the hub. */
export async function peersOverHub(
  node: ScenarioNode,
  peer: ScenarioNode,
): Promise<boolean> {
  const { peers } = await node.rpc.call("listPeers", {});
  return peers.some((p) =>
    p.node === peer.id && p.scheme === HUB_SCHEME && p.state === "peered"
  );
}

/** Whether `node` follows the hub and has an account in it with funds. */
export async function fundedInHub(node: ScenarioNode): Promise<boolean> {
  const view = await ledger(node, HUB_LEDGER);
  return view.connection === LedgerConnection.SYNCED && view.ownerBalance > 0n;
}

/** Asserts that `node` peers over the hub alone: its owner book peers with
 * nobody. */
export async function assertPeersOverHubAlone(
  node: ScenarioNode,
): Promise<void> {
  const { peers } = await node.rpc.call("listPeers", {});
  for (const peer of peers) assertEquals(peer.scheme, HUB_SCHEME);
}

/**
 * Waits until the members of `scenario`'s hub, which the scenario founded
 * as it started, joined it: each opened its account in the hub by a
 * deposit from its owner book, by itself (spec/areas/hubs.md#in-the-node),
 * every two of them peer over the hub alone, and settling is over.
 */
export async function joinedHub(scenario: Scenario): Promise<void> {
  const nodes = members(scenario);
  await scenario.until("members funded in the hub", async () => {
    for (const node of nodes) if (!(await fundedInHub(node))) return false;
    return true;
  }, HUB_WITHIN_MS);
  await scenario.until("peered over the hub", async () => {
    for (const node of nodes) {
      for (const peer of nodes) {
        if (peer !== node && !(await peersOverHub(node, peer))) return false;
      }
    }
    return true;
  }, HUB_WITHIN_MS);
  for (const node of nodes) await assertPeersOverHubAlone(node);
  await scenario.until(
    "in step",
    () => settledOverHub(scenario),
    HUB_WITHIN_MS,
  );
}
