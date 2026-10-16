/**
 * The M7c done criterion (spec 7 §8): the `cross-currency` scenario
 * settles and reconciles across restarts. d1 has dollars (stub), d3 has
 * XRP (the fake ledger), and d2 has both, charging a fee and a spread on
 * what it passes between them. The rates are the scenario's own.
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
/** What a node extends to each peer, on either ledger. */
const CREDIT = 1_100_000_000n;
/** d2's fee, 1%, and spread, 0.5%. */
const FEE_PPM = 10_000;
const SPREAD_PPM = 5_000;
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

/** Every peer account, net credit, by `node>peer@ledger`. */
async function peerAccounts(scenario: Scenario): Promise<Map<string, bigint>> {
  const byId = new Map(scenario.nodes.map((node) => [node.id, node.name]));
  const accounts = new Map<string, bigint>();
  for (const node of scenario.nodes) {
    const list = await node.rpc.call("listAccounts", {});
    for (const account of list.accounts) {
      const match = /^(.+):assets\/interledger\/(.+)$/.exec(account.path);
      if (!match) continue;
      const peer = byId.get(match[2]) ?? match[2];
      accounts.set(
        `${node.name}>${peer}@${match[1]}`,
        account.creditsPosted - account.debitsPosted,
      );
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
    if (theirs === undefined || mine + theirs !== 2n * CREDIT) return false;
  }
  return accounts.size === 4;
}

/** Settling went quiet and stays so: a settlement that is due but not
 * started yet shows only in a while, as the books move. */
async function settled(scenario: Scenario): Promise<boolean> {
  if (!(await quiet(scenario))) return false;
  const before = await books(scenario);
  await new Promise((resolve) => setTimeout(resolve, 10_000));
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
        graceSharePpm: 750_000,
        connectorFeePpm: FEE_PPM,
        fxSpreadPpm: SPREAD_PPM,
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
    // 2 XRP a dollar, less d2's 1% and 0.5%, and STREAM's 1%: a little
    // over 2.05 dollars.
    assert(
      quote.debitAmount > 2_050_000_000n &&
        quote.debitAmount < 2_060_000_000n,
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
    const { payments: received } = await d3.rpc.call(
      "listIncomingPayments",
      {},
    );
    const incoming = received.find((entry) =>
      entry.metadataJson.includes("e2e across")
    );
    assert(incoming, "d3 has the payment");
    // All of it, but what less than a dollar unit is worth: no packet
    // can be sized to deliver that, as the hops round down.
    assert(
      incoming.receivedAmount >= RECEIVE - 2n,
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
