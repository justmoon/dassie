/**
 * End to end (spec/areas/testing.md#end-to-end): the `cross-currency`
 * scenario settles and reconciles across restarts. d1 has dollars (stub),
 * d3 has XRP (the fake ledger), and d2 has both, charging a fee and a
 * spread on what it passes between them, and pricing it by what it holds
 * (spec/decisions/passive-rebalancing.md). The rates are the scenario's
 * own.
 *
 * d1 pays d3's payment pointer, whose ledger is XRP: the quote measures
 * the path's rate, d1 pays from its dollars, d2 converts them, and d3
 * receives XRP. d2's fees grow; every node settles with its peers, on
 * each one's ledger; after all three restart, the books are as they were.
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
import { Scenario, type ScenarioNode } from "@dassie/dev/scenario";

const USD = "stub+usd";
const XRP = "xrpl-testnet+xrp";
/** 4 XRP: more than a peer's credit, so every node must settle. */
const RECEIVE = 4_000_000_000n;
/**
 * How far from 4 XRP d3's incoming payment may land and be paid in full: a
 * ten-thousandth of an XRP, at scale 9
 * (spec/decisions/paid-within-a-tolerance.md). The hops round down on
 * every packet, and packets in flight together race: when one that leaves
 * d3 short by less than this lands before the last, d3 completes the
 * payment there and refuses the last.
 */
const TOLERANCE = 100_000n;
/** d2's fee, 1%, and spread, 0.5%. */
const FEE_PPM = 10_000;
const SPREAD_PPM = 5_000;
/** d2's k, 5%, the curve every node starts with. */
const REBALANCE_PPM = 50_000;
const PORT_OFFSET = SCENARIO_PORT_OFFSET + 500;

const missing = await missingScenarioTools();

/** A node's ledger, if it has it. */
async function ledger(node: ScenarioNode, id: string) {
  const { ledgers } = await node.rpc.call("listLedgers", {});
  return ledgers.find((entry) => entry.id === id);
}

/** d2 and d3 follow the XRP Ledger, funded, and d2 has a fresh rate. */
async function ready(scenario: Scenario): Promise<boolean> {
  const [, d2, d3] = scenario.nodes;
  for (const node of [d2, d3]) {
    const view = await ledger(node, XRP);
    if (view?.connection !== LedgerConnection.SYNCED) return false;
    if (view.ownerBalance <= 0n) return false;
  }
  const { rates } = await d2.rpc.call("listLedgers", {});
  return rates.some((rate) => rate.assetCode === "XRP" && !rate.stale);
}

/** A node's balance in one of its accounts, by path. */
async function balance(node: ScenarioNode, path: string): Promise<bigint> {
  const { accounts } = await node.rpc.call("listAccounts", {});
  const account = accounts.find((entry) => entry.path === path);
  return account ? account.creditsPosted - account.debitsPosted : 0n;
}

/**
 * What each node owes each peer net, by its own books, by
 * `node>peer@ledger`: its peer account less the credit it extends,
 * negative when the peer owes it.
 */
async function peerAccounts(scenario: Scenario): Promise<Map<string, bigint>> {
  const byId = new Map(scenario.nodes.map((node) => [node.id, node.name]));
  const accounts = new Map<string, bigint>();
  for (const node of scenario.nodes) {
    const list = await node.rpc.call("listAccounts", {});
    for (const account of list.accounts) {
      const match = /^(.+):(assets\/interledger|contra\/trust)\/(.+)$/
        .exec(account.path);
      if (!match) continue;
      const peer = byId.get(match[3]) ?? match[3];
      const key = `${node.name}>${peer}@${match[1]}`;
      const net = account.creditsPosted - account.debitsPosted;
      accounts.set(key, (accounts.get(key) ?? 0n) + net);
    }
  }
  return accounts;
}

/** Settling went quiet: nothing on its way, the ledgers followed, and
 * each pair of books agrees. */
async function quiet(scenario: Scenario): Promise<boolean> {
  for (const node of scenario.nodes) {
    const { channels } = await node.rpc.call("listSettlementChannels", {});
    if (channels.some((channel) => channel.sending)) return false;
    const view = await ledger(node, XRP);
    if (view && view.connection !== LedgerConnection.SYNCED) return false;
  }
  const accounts = await peerAccounts(scenario);
  for (const [key, mine] of accounts) {
    const [pair, on] = key.split("@");
    const [node, peer] = pair.split(">");
    const theirs = accounts.get(`${peer}>${node}@${on}`);
    if (theirs === undefined || mine + theirs !== 0n) return false;
  }
  return accounts.size === 4;
}

/**
 * Longer than a node's settlement interval (4 s, 1 s with dev timings): a
 * settlement that is due starts within it.
 */
const SETTLE_CHECK_MS = 5_000;

/** Settling went quiet and stays so: a settlement that is due but not
 * started yet shows only in a while, as the books move. */
async function settled(scenario: Scenario): Promise<boolean> {
  if (!(await quiet(scenario))) return false;
  const before = await books(scenario);
  await new Promise((resolve) => setTimeout(resolve, SETTLE_CHECK_MS));
  const after = await books(scenario);
  return (await quiet(scenario)) &&
    [...before].every(([key, value]) => after.get(key) === value);
}

/** What each node's books hold: per ledger, on the ledger and the
 * owner's. */
async function books(scenario: Scenario) {
  const out = new Map<string, string>();
  for (const node of scenario.nodes) {
    const { ledgers } = await node.rpc.call("listLedgers", {});
    for (const entry of ledgers) {
      out.set(
        `${node.name}@${entry.id}`,
        `${entry.onLedgerBalance}/${entry.ownerBalance}`,
      );
    }
  }
  return out;
}

test("cross-currency pays across assets, and reconciles across restarts", {
  skip: missing !== undefined,
}, async () => {
  const scenario = await Scenario.temporary("cross-currency", {
    dassie: DASSIE,
    fakeLedger: FAKE_LEDGER,
    portOffset: PORT_OFFSET,
  });
  try {
    const [d1, d2, d3] = scenario.nodes;
    await scenario.until("funded", () => ready(scenario), 120_000);
    await d2.rpc.call("setPeerSettings", {
      settings: {
        connectorFeePpm: FEE_PPM,
        fxSpreadPpm: SPREAD_PPM,
        rebalancePpm: REBALANCE_PPM,
      },
    });

    // d1 quotes 4 XRP to d3, once d3 is routed to.
    const pointer = `$${d3.hostname}:${d3.httpsPort}`;
    const quoted: { value?: SendQuote } = {};
    await scenario.until("d1 quotes d3", async () => {
      try {
        quoted.value = (await d1.rpc.call("quoteSend", {
          recipient: pointer,
          ledger: "",
          amount: { case: "receiveAmount", value: RECEIVE },
          note: "e2e across",
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
    // d2 holds 100 dollars and 999 XRP, worth 499.5: dollars are a sixth
    // of what it holds, far short of half, so it takes them in at the most
    // the curve gives, 5% above spot. 2 XRP a dollar, 5% more, less d2's
    // 1% and 0.5%, and STREAM's 1%: a little over 1.95 dollars. The
    // payment moves d2's holdings by a few dollars, which keeps the bonus
    // at its cap.
    assert(
      quote.debitAmount > 1_950_000_000n &&
        quote.debitAmount < 1_960_000_000n,
      `${quote.debitAmount}`,
    );

    const usdBefore = (await ledger(d1, USD))!.ownerBalance;
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
    // went back (spec/decisions/own-fulfills.md). The one the payment paid
    // is its receiver: a quote tried before d3 was routed to leaves an
    // incoming payment of its own, with the same note and nothing in it.
    const arrived = async () => {
      const { payments: received } = await d3.rpc.call(
        "listIncomingPayments",
        {},
      );
      return received.find((entry) => entry.url === sent.receiver);
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
    // d1 paid in dollars; d2 kept its fee in dollars.
    assertEquals(
      (await ledger(d1, USD))!.ownerBalance,
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

    await scenario.until("settled", () => settled(scenario), 180_000);
    // Restarted, all three catch up and book nothing twice.
    const kept = await books(scenario);
    const accounts = await peerAccounts(scenario);
    for (const node of scenario.nodes) {
      const { code, output } = (await node.stop())!;
      assertEquals(code, 0, output);
    }
    for (const node of scenario.nodes) {
      await node.start();
      await node.waitForEngine();
    }
    await scenario.until("caught up", () => quiet(scenario), 120_000);
    assertEquals(await books(scenario), kept, "the books after a restart");
    assertEquals(await peerAccounts(scenario), accounts, "peer accounts");
  } finally {
    const failures = await scenario.stop();
    assertEquals(failures, [], failures.join("\n"));
  }
});
