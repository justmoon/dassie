/**
 * End to end (spec/areas/testing.md#end-to-end): the `cross-currency`
 * scenario pays across assets and reconciles across restarts, settling
 * nothing directly. d1 has dollars on the stub with unlimited trust, which
 * never settles; d3 has XRP in the scenario's settlement hub, which four
 * validators found on the fake ledger; and d2 has both, peering with d1 on
 * the stub and with d3 over the hub alone. d2 charges a fee and a spread
 * on what it passes between them, at spot (its rebalancing curve off, so
 * that what the hub holds of its value does not move the price). The
 * rates are the scenario's own.
 *
 * d2 and d3 join the hub by a deposit from their owner books of XRP, which
 * peer with nobody. d1 pays an incoming payment of d3's on the hub's
 * ledger: the quote measures the path's rate, d1 pays from its dollars, d2
 * converts them, and d3 receives XRP over the hub. d2's fees grow; d2
 * settles with d3 over the hub, and d1 and d2 settle nothing; after all
 * three restart, the books are as they were. The validators keep running
 * meanwhile, so the hub does too.
 */

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { LedgerConnection, type SendQuote } from "@dassie/rpc/proto";

import {
  DASSIE,
  FAKE_LEDGER,
  missingScenarioTools,
  SCENARIO_PORT_OFFSET,
} from "./harness.ts";
import {
  fundedInHub,
  HUB_WITHIN_MS,
  hubBooks,
  ledger,
  peerAccounts,
  peersOverHub,
  settledOverHub,
} from "./settling.ts";
import {
  HUB_LEDGER,
  HUB_SCHEME,
  Scenario,
  type ScenarioNode,
} from "@dassie/dev/scenario";

const USD = "stub+usd";
/** What the stub holds when it starts: 100 dollars. */
const STUB_START = 100_000_000_000n;
/** 2 XRP: twenty of the largest packets, so d2 funds its hold toward d3
 * over the hub as it pays. */
const RECEIVE = 2_000_000_000n;
/**
 * How far from 2 XRP d3's incoming payment may land and be paid in full: a
 * ten-thousandth of an XRP, at scale 9
 * (spec/decisions/paid-within-a-tolerance.md). The hops round down on
 * every packet, and packets in flight together race: when one that leaves
 * d3 short by less than this lands before the last, d3 completes the
 * payment there and refuses the last.
 */
const TOLERANCE = 100_000n;
/** d2's fee, 1%, and spread, 0.5%; its curve off, so it converts at spot
 * less them. */
const FEE_PPM = 10_000;
const SPREAD_PPM = 5_000;
const REBALANCE_PPM = 0;
const PORT_OFFSET = SCENARIO_PORT_OFFSET + 500;

const missing = await missingScenarioTools();

/** Whether `node` peers with `peer` on the stub. */
async function peersOnStub(
  node: ScenarioNode,
  peer: ScenarioNode,
): Promise<boolean> {
  const { peers } = await node.rpc.call("listPeers", {});
  return peers.some((p) =>
    p.node === peer.id && p.scheme === "stub" && p.state === "peered"
  );
}

/**
 * d2 and d3 joined the hub and peer over it, d1 and d2 peer on the stub,
 * and d2 has a fresh rate for XRP.
 */
async function ready(scenario: Scenario): Promise<boolean> {
  const [d1, d2, d3] = scenario.nodes;
  for (const node of [d2, d3]) if (!(await fundedInHub(node))) return false;
  if (!(await peersOnStub(d1, d2)) || !(await peersOnStub(d2, d1))) {
    return false;
  }
  if (!(await peersOverHub(d2, d3)) || !(await peersOverHub(d3, d2))) {
    return false;
  }
  const { rates } = await d2.rpc.call("listLedgers", {});
  return rates.some((rate) => rate.assetCode === "XRP" && !rate.stale);
}

/** What `node` holds for `peer` over the hub: the credit it extends the
 * peer there (spec/areas/hubs.md#in-the-node). */
async function holdToward(
  node: ScenarioNode,
  peer: ScenarioNode,
): Promise<bigint> {
  const { peers } = await node.rpc.call("listPeers", {});
  const found = peers.find((entry) =>
    entry.node === peer.id && entry.scheme === HUB_SCHEME
  );
  assert(found, `${node.name} does not peer with ${peer.name} over the hub`);
  return found.creditOut;
}

/** A node's balance in one of its accounts, by path. */
async function balance(node: ScenarioNode, path: string): Promise<bigint> {
  const { accounts } = await node.rpc.call("listAccounts", {});
  const account = accounts.find((entry) => entry.path === path);
  return account ? account.creditsPosted - account.debitsPosted : 0n;
}

/**
 * What the stub's ledger holds of d1 and of d2, on the ledger and the
 * owner's, with nothing in flight: the stub with unlimited trust never
 * settles, so on the ledger each holds what it started with.
 */
async function stubBooks(scenario: Scenario) {
  const [d1, d2] = scenario.nodes;
  return await Promise.all([d1, d2].map(async (node) => {
    const view = await ledger(node, USD);
    assertEquals(view.connection, LedgerConnection.SYNCED, node.name);
    assertEquals(view.onLedgerBalance, STUB_START, `${node.name} settled`);
    return [view.onLedgerBalance, view.ownerBalance, view.ownerPending];
  }));
}

/** What d1, d2 and d3 owe each other, by their own books, on the stub and
 * over the hub. */
async function owed(scenario: Scenario) {
  return {
    stub: await peerAccounts(scenario, USD),
    hub: await peerAccounts(scenario, HUB_LEDGER),
  };
}

test(
  "cross-currency pays across assets over a hub, and reconciles across restarts",
  {
    skip: missing !== undefined,
  },
  async () => {
    const scenario = await Scenario.temporary("cross-currency", {
      dassie: DASSIE,
      fakeLedger: FAKE_LEDGER,
      portOffset: PORT_OFFSET,
    });
    try {
      const [d1, d2, d3] = scenario.nodes;
      await scenario.until("joined and peered", () => ready(scenario), 240_000);
      // d1 and d3 share no scheme, so they never peer; d3 peers over the hub
      // alone.
      const { peers } = await d3.rpc.call("listPeers", {});
      for (const peer of peers) assertEquals(peer.scheme, HUB_SCHEME);
      assert(!peers.some((peer) => peer.node === d1.id), "d1 peers with d3");
      await d2.rpc.call("setPeerSettings", {
        settings: {
          connectorFeePpm: FEE_PPM,
          fxSpreadPpm: SPREAD_PPM,
          rebalancePpm: REBALANCE_PPM,
        },
      });

      // d1 quotes 2 XRP to d3's incoming payment on the hub's ledger, once
      // d3 is routed to.
      const { payment: receiver } = await d3.rpc.call("createIncomingPayment", {
        ledger: HUB_LEDGER,
        incomingAmount: RECEIVE,
        description: "e2e across",
      });
      assert(receiver);
      assertEquals(receiver.ledger, HUB_LEDGER);
      const quoted: { value?: SendQuote } = {};
      await scenario.until("d1 quotes d3", async () => {
        try {
          quoted.value = (await d1.rpc.call("quoteSend", {
            recipient: "",
            credentials: {
              destinationAccount: receiver.destinationAccount,
              sharedSecret: receiver.sharedSecret,
              assetCode: receiver.assetCode,
              assetScale: receiver.assetScale,
            },
            ledger: "",
            amount: { case: "receiveAmount", value: RECEIVE },
          })).quote;
          return true;
        } catch {
          return false;
        }
      }, 120_000);
      const quote = quoted.value;
      assert(quote);
      assertEquals(quote.ledger, USD);
      assertEquals(quote.receiveAmount?.assetCode, "XRP");
      assertEquals(quote.receiveAmount?.value, RECEIVE);
      // 2 XRP are a dollar at spot; less d2's 1% and 0.5%, and STREAM's 1%:
      // a little over 1.025 dollars.
      assert(
        quote.debitAmount > 1_020_000_000n &&
          quote.debitAmount < 1_030_000_000n,
        `${quote.debitAmount}`,
      );

      const holdBefore = await holdToward(d2, d3);
      const d3HubBefore = (await ledger(d3, HUB_LEDGER)).onLedgerBalance;
      const usdBefore = (await ledger(d1, USD)).ownerBalance;
      const feesBefore = await balance(d2, `${USD}:revenue/fees`);
      const { payment } = await d1.rpc.call("sendPayment", {
        quoteId: quote.id,
      });
      assert(payment);
      await scenario.until("the payment completes", async () => {
        const { payments } = await d1.rpc.call("listOutgoingPayments", {});
        const found = payments.find((entry) => entry.id === payment.id);
        assert(found?.state !== "failed", found?.failure);
        return found?.state === "completed";
      }, 240_000);
      const { payments } = await d1.rpc.call("listOutgoingPayments", {});
      const sent = payments.find((entry) => entry.id === payment.id)!;
      // d3 counted each packet received as it posted it, before its Fulfill
      // went back (spec/decisions/own-fulfills.md).
      const arrived = async () => {
        const { payments: received } = await d3.rpc.call(
          "listIncomingPayments",
          {},
        );
        return received.find((entry) => entry.id === receiver.id);
      };
      await scenario.until(
        "d3 has it all",
        async () => (await arrived())?.completedAtUnixMs !== undefined,
      );
      const incoming = await arrived();
      assert(incoming, "d3 has the payment");
      // All of it, within the tolerance: d3 completed it, so it counts it
      // paid in full.
      assert(
        incoming.receivedAmount >= RECEIVE - TOLERANCE &&
          incoming.receivedAmount <= RECEIVE + TOLERANCE,
        `${incoming.receivedAmount}`,
      );
      assertEquals(incoming.assetCode, "XRP");
      assertEquals(incoming.ledger, HUB_LEDGER);
      // d1 paid in dollars; d2 kept its fee in dollars.
      assertEquals(
        (await ledger(d1, USD)).ownerBalance,
        usdBefore - sent.sentAmount,
      );
      // 1% of what passed, less under a unit a packet, as each rounds
      // down.
      const fees = (await balance(d2, `${USD}:revenue/fees`)) - feesBefore;
      const due = sent.sentAmount * BigInt(FEE_PPM) / 1_000_000n;
      assert(
        fees <= due && fees > due - 1_000n,
        `fees ${fees} on ${sent.sentAmount}`,
      );

      // d2 settles with d3 over the hub; d1 and d2 settle nothing on the
      // stub, and their books mirror each other.
      await scenario.until(
        "settled over the hub",
        () => settledOverHub(scenario),
        HUB_WITHIN_MS,
      );
      // Over the hub: d2's hold toward d3 grew, and d3's book of the hub
      // holds what it grew by, which never passes what d3 received.
      const moved = await holdToward(d2, d3) - holdBefore;
      assert(
        moved > 0n && moved <= incoming.receivedAmount,
        `d2's hold toward d3 grew by ${moved}`,
      );
      assertEquals(
        (await ledger(d3, HUB_LEDGER)).onLedgerBalance - d3HubBefore,
        moved,
        "d3's book of the hub",
      );
      const stub = await stubBooks(scenario);
      const accounts = await owed(scenario);
      const d1Owes = accounts.stub.get("d1>d2");
      assert(d1Owes !== undefined, "d1 has no account of d2");
      assertEquals(accounts.stub.get("d2>d1"), -d1Owes, "the stub's books");
      assertEquals(d1Owes, sent.sentAmount, "d1 owes d2 what it sent");

      // Restarted, all three catch up and book nothing twice.
      const hub = await Promise.all([d2, d3].map(hubBooks));
      for (const node of [d1, d2, d3]) {
        const { code, output } = (await node.stop())!;
        assertEquals(code, 0, output);
      }
      for (const node of [d1, d2, d3]) {
        await node.start();
        await node.waitForEngine();
      }
      await scenario.until(
        "caught up",
        () => settledOverHub(scenario),
        HUB_WITHIN_MS,
      );
      assertEquals(
        await Promise.all([d2, d3].map(hubBooks)),
        hub,
        "the hub's books after a restart",
      );
      assertEquals(await stubBooks(scenario), stub, "the stub after a restart");
      assertEquals(await owed(scenario), accounts, "peer accounts");
    } finally {
      const failures = await scenario.stop();
      assertEquals(failures, [], failures.join("\n"));
    }
  },
);
