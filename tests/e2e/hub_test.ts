/**
 * End to end (spec/areas/testing.md#end-to-end): the `hub` scenario's
 * nodes found a settlement hub on the fake XRP Ledger, and its members
 * settle over it. Four validators run the hub their sponsor founds through
 * its owner's RPC; the two members fund their accounts in it from their
 * owner books of XRP, which peer with nobody, and peer over the hub alone.
 *
 * A client of one member pays the other's receiver twenty of the largest
 * packets, so the payer funds its hold toward the payee over the hub as it
 * pays; what arrived is what was sent, each member's books hold what its
 * accounts hold, and both members' books are unchanged once they restart.
 * The validators keep running meanwhile, so the hub does too.
 */

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { LedgerConnection } from "@dassie/rpc/proto";

import { DASSIE, FAKE_LEDGER, missingScenarioTools } from "./harness.ts";
import { ledger, pay, peerAccounts } from "./settling.ts";
import {
  HUB_LEDGER,
  HUB_OWNER_BOOK,
  HUB_SCHEME,
  Scenario,
  type ScenarioNode,
} from "@dassie/dev/scenario";

/** The members' owner book of XRP, from which they fund the hub. */
const OWNER_LEDGER = `${HUB_OWNER_BOOK}+xrp`;
/** 2 XRP: twenty of the largest packets, so the payer funds its hold
 * toward the payee over the hub as it pays. */
const PAYMENT = 2_000_000_000n;
/** Internal units in a drop: XRP's ledgers are kept at scale 9, a drop at
 * scale 6. */
const UNITS_PER_DROP = 1_000n;
/**
 * Clear of every other file's ports, which run beside these, as
 * market_maker_targets_test.ts lays them out: at 7000 this file's nodes
 * take 11001 up and 12001 up on TCP, 12001 up for QUIC and 13001 up for
 * their feeds on UDP, and its fake ledger and rate server 17200, 17201 and
 * 17210 on TCP.
 */
const PORT_OFFSET = 7_000;
/** Longer than a node's settlement interval with dev timings. */
const SETTLE_CHECK_MS = 5_000;
/**
 * How long the hub's members may take to open their accounts, peer over
 * it, or settle: each move and operation waits for a validated ledger and
 * a final block, and a member reads the hub again after a restart.
 */
const HUB_WITHIN_MS = 180_000;

const missing = await missingScenarioTools();

/** The members of the scenario's hub. */
function members(scenario: Scenario): ScenarioNode[] {
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

/** What `node`'s books hold of each of its two ledgers, and what its owner
 * has there. */
async function books(node: ScenarioNode) {
  const [hub, owner] = await Promise.all([
    ledger(node, HUB_LEDGER),
    ledger(node, OWNER_LEDGER),
  ]);
  return {
    hub: [hub.onLedgerBalance, hub.ownerBalance],
    owner: [owner.onLedgerBalance, owner.ownerBalance],
  };
}

/** What `node`'s owner has on both ledgers. */
async function ownerFunds(node: ScenarioNode): Promise<bigint> {
  const { hub, owner } = await books(node);
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
 * extends it.
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
    [OWNER_LEDGER, HUB_OWNER_BOOK, 0n],
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
 * Settling is over: every member in step, and still in step with the
 * same books once a settlement interval has passed.
 */
async function settled(scenario: Scenario): Promise<boolean> {
  const nodes = members(scenario);
  for (const node of nodes) if (!(await inStep(scenario, node))) return false;
  const first = JSON.stringify(await Promise.all(nodes.map(books)), text);
  await new Promise((resolve) => setTimeout(resolve, SETTLE_CHECK_MS));
  for (const node of nodes) if (!(await inStep(scenario, node))) return false;
  return JSON.stringify(await Promise.all(nodes.map(books)), text) === first;
}

/** JSON of bigints, as text. */
function text(_key: string, value: unknown) {
  return typeof value === "bigint" ? `${value}` : value;
}

/** Whether `node` peers with `peer` over the hub. */
async function peersOverHub(
  node: ScenarioNode,
  peer: ScenarioNode,
): Promise<boolean> {
  const { peers } = await node.rpc.call("listPeers", {});
  return peers.some((p) =>
    p.node === peer.id && p.scheme === HUB_SCHEME && p.state === "peered"
  );
}

test(
  "hub founds a settlement hub, and its members settle over it across restarts",
  { skip: missing !== undefined },
  async () => {
    const scenario = await Scenario.temporary("hub", {
      dassie: DASSIE,
      fakeLedger: FAKE_LEDGER,
      portOffset: PORT_OFFSET,
    });
    try {
      const [payer, payee] = members(scenario);
      // Each member opens its account in the hub by a deposit from its
      // owner book, by itself (spec/areas/hubs.md#in-the-node).
      await scenario.until("members funded in the hub", async () => {
        for (const node of [payer, payee]) {
          const view = await ledger(node, HUB_LEDGER);
          if (view.connection !== LedgerConnection.SYNCED) return false;
          if (view.ownerBalance <= 0n) return false;
        }
        return true;
      }, HUB_WITHIN_MS);
      await scenario.until(
        "peered over the hub",
        async () =>
          (await peersOverHub(payer, payee)) &&
          (await peersOverHub(payee, payer)),
        HUB_WITHIN_MS,
      );
      // Their owner books peer with nobody.
      for (const node of [payer, payee]) {
        const { peers } = await node.rpc.call("listPeers", {});
        for (const peer of peers) assertEquals(peer.scheme, HUB_SCHEME);
      }
      await scenario.until("in step", () => settled(scenario), HUB_WITHIN_MS);

      const [paid, received] = await Promise.all(
        [payer, payee].map(ownerFunds),
      );
      const address = await pay(
        scenario,
        [payer, payee],
        HUB_LEDGER,
        PAYMENT,
        HUB_LEDGER,
      );
      const { payments } = await payee.rpc.call("listIncomingPayments", {});
      const incoming = payments.find((p) => p.destinationAccount === address);
      assert(incoming, "no incoming payment");
      assertEquals(incoming.ledger, HUB_LEDGER);
      assertEquals(incoming.receivedAmount, PAYMENT, "what arrived");
      await scenario.until("settled", () => settled(scenario), HUB_WITHIN_MS);
      // The payer's owner paid what was sent and the fees of its moves and
      // operations; the payee's got what arrived, less those of its own.
      const spent = paid - await ownerFunds(payer);
      const got = await ownerFunds(payee) - received;
      assert(spent >= PAYMENT, `the payer's owner spent ${spent}`);
      assert(got > 0n && got <= PAYMENT, `the payee's owner got ${got}`);

      // Restarted, both members read their accounts again and book nothing
      // twice.
      const kept = await Promise.all([payer, payee].map(books));
      const accounts = await peerAccounts(scenario, HUB_LEDGER);
      for (const node of [payer, payee]) {
        const { code, output } = (await node.stop())!;
        assertEquals(code, 0, output);
      }
      for (const node of [payer, payee]) {
        await node.start();
        await node.waitForEngine();
      }
      await scenario.until(
        "caught up",
        () => settled(scenario),
        HUB_WITHIN_MS,
      );
      assertEquals(
        await Promise.all([payer, payee].map(books)),
        kept,
        "the books after a restart",
      );
      assertEquals(
        await peerAccounts(scenario, HUB_LEDGER),
        accounts,
        "peer accounts",
      );
    } finally {
      const failures = await scenario.stop();
      assertEquals(failures, [], failures.join("\n"));
    }
  },
);
