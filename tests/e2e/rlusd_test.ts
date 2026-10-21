/**
 * End to end (spec/areas/testing.md#end-to-end): the `rlusd-settlement`
 * scenario settles in RLUSD (spec/areas/xrpl.md#rlusd). The nodes are the
 * real binary; the XRP Ledger is the node's fake, over WebSocket, with its
 * faucet and the issuer of its test RLUSD over HTTPS: only the fake mints
 * test RLUSD on demand, and only it lets a test freeze and claw back.
 *
 * The faucet gives both nodes XRP for gas, each sets its trust line, and
 * the issuer mints them RLUSD. A client of d1 pays d2's receiver several
 * times the credit, so d1 settles in RLUSD as it pays. Then the issuer
 * freezes d2's line, which the owner sees, and claws back some of d2's
 * RLUSD, which d2 books as a withdrawal: each node's books stay in step
 * with its line.
 */

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { Buffer } from "node:buffer";
import { readFile } from "node:fs/promises";
import WebSocket from "#ws";
import { createConnection } from "ilp-protocol-stream";
import { LedgerConnection } from "@dassie/rpc/proto";

import {
  DASSIE,
  FAKE_LEDGER,
  missingScenarioTools,
  SCENARIO_PORT_OFFSET,
} from "./harness.ts";
import { BtpClient } from "./btp.ts";
import { Scenario, type ScenarioNode } from "@dassie/dev/scenario";

const LEDGER = "xrpl-testnet-rlusd+usd";
/** What the scenario's issuer mints each node: 100 dollars. */
const MINTED = 100_000_000_000n;
/** 5 dollars: over four times what a peer extends, so d1 must settle. */
const PAYMENT = 5_000_000_000n;
/** What a node extends to each peer. */
const CREDIT = 1_100_000_000n;
/** Every node's trigger: one and a half of the largest packets (spec/decisions/thresholds-in-packets.md). */
const TRIGGER = 150_000_000n;
/** What the issuer claws back from d2: 2 dollars. */
const CLAWED = 2_000_000_000n;
/** The ledger's unit, a millionth of a dollar, in internal units. */
const UNIT = 1_000n;
const PORT_OFFSET = SCENARIO_PORT_OFFSET + 900;

const missing = await missingScenarioTools();

/** A node's RLUSD ledger, as its owner sees it. */
async function ledger(node: ScenarioNode) {
  const { ledgers } = await node.rpc.call("listLedgers", {});
  const found = ledgers.find((l) => l.id === LEDGER);
  assert(found, `${node.name} has no ${LEDGER}`);
  return found;
}

/** What a node's account holds of RLUSD, as the ledger says, in the
 * ledger's internal units: whole millionths, rounded down. */
async function line(node: ScenarioNode) {
  const { externalAssets } = await node.rpc.call("listLedgers", {});
  const holding = externalAssets
    .flatMap((asset) => asset.holdings)
    .find((holding) => holding.assetCode === "RLUSD");
  assert(holding, `${node.name} shows no RLUSD line`);
  const scale = BigInt(holding.assetScale);
  const micros = holding.amount / 10n ** (scale - 6n);
  return { holding, units: micros * UNIT };
}

/** Whether a node follows the ledger, and its books hold its line. */
async function inStep(node: ScenarioNode): Promise<boolean> {
  const view = await ledger(node);
  if (view.connection !== LedgerConnection.SYNCED) return false;
  return view.onLedgerBalance === (await line(node)).units;
}

/** Whether every node is in step, with the RLUSD the issuer minted it. */
async function funded(scenario: Scenario): Promise<boolean> {
  for (const node of scenario.nodes) {
    if (!(await inStep(node))) return false;
    if ((await ledger(node)).ownerBalance < MINTED) return false;
  }
  return true;
}

/**
 * What each node owes each peer net, by its own books, by `node>peer`: its
 * peer account less the credit it extends, negative when the peer owes it.
 */
async function peerAccounts(scenario: Scenario): Promise<Map<string, bigint>> {
  const byId = new Map(scenario.nodes.map((node) => [node.id, node.name]));
  const accounts = new Map<string, bigint>();
  for (const node of scenario.nodes) {
    const list = await node.rpc.call("listAccounts", {});
    for (const account of list.accounts) {
      for (const kind of ["assets/interledger", "contra/trust"]) {
        const prefix = `${LEDGER}:${kind}/`;
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
 * Settling went quiet: nothing on its way, every node in step with its
 * line, and no node's books showing a peer at its trigger.
 */
async function quiet(scenario: Scenario): Promise<boolean> {
  for (const node of scenario.nodes) {
    const { channels } = await node.rpc.call("listSettlementChannels", {});
    if (channels.some((channel) => channel.sending)) return false;
    if (!(await inStep(node))) return false;
    const { peers } = await node.rpc.call("listPeers", {});
    if (peers.some((peer) => peer.creditOut + peer.owed <= TRIGGER)) {
      return false;
    }
  }
  return (await peerAccounts(scenario)).size > 0;
}

/** Longer than a node's settlement interval with dev timings. */
const SETTLE_CHECK_MS = 5_000;

/** Settling is over: quiet, and still quiet with the same books once a
 * settlement interval has passed. */
async function settled(scenario: Scenario): Promise<boolean> {
  if (!(await quiet(scenario))) return false;
  const first = await Promise.all(scenario.nodes.map(ledger));
  await new Promise((resolve) => setTimeout(resolve, SETTLE_CHECK_MS));
  if (!(await quiet(scenario))) return false;
  const second = await Promise.all(scenario.nodes.map(ledger));
  return first.every((view, index) =>
    view.onLedgerBalance === second[index].onLedgerBalance &&
    view.ownerBalance === second[index].ownerBalance
  );
}

/** The withdrawals a node booked on the ledger. */
async function withdrawals(node: ScenarioNode): Promise<bigint[]> {
  const { transfers } = await node.rpc.call("listTransfers", { limit: 1000 });
  return transfers
    .filter((transfer) =>
      transfer.reference.case === "other" &&
      transfer.reference.value === "withdrawal" &&
      transfer.debitPath.startsWith(`${LEDGER}:`)
    )
    .map((transfer) => transfer.amount);
}

test(
  "rlusd-settlement settles in RLUSD, and books the issuer's clawback as a withdrawal",
  { skip: missing !== undefined },
  async () => {
    const scenario = await Scenario.temporary("rlusd-settlement", {
      dassie: DASSIE,
      fakeLedger: FAKE_LEDGER,
      portOffset: PORT_OFFSET,
    });
    try {
      await scenario.until("funded", () => funded(scenario), 120_000);
      await scenario.until("peered", () => scenario.converged(), 120_000);
      const [d1, d2] = scenario.nodes;
      // The ledger names the issuer whose RLUSD to send: its line's.
      for (const node of scenario.nodes) {
        const { holding } = await line(node);
        assertEquals((await ledger(node)).issuer, holding.issuer);
      }
      // The junior pays its senior in advance before it can send through
      // it (spec/decisions/earned-trust.md).
      await scenario.until("paid in advance", () => settled(scenario), 120_000);

      const before = await ledger(d1);
      const token = await d1.rpc.call("createBtpToken", {
        label: "e2e",
        ledger: LEDGER,
      });
      const { payment: credentials } = await d2.rpc.call(
        "createIncomingPayment",
        {},
      );
      assert(credentials);
      const server = token.url.replace(`@${d1.hostname}:`, "@localhost:");
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
        await stream.sendTotal(Number(PAYMENT), { timeout: 240_000 });
        assertEquals(BigInt(stream.totalSent), PAYMENT);
        await connection.end();
      } finally {
        await plugin.disconnect();
      }
      await scenario.until("settled", () => settled(scenario), 120_000);
      // d1 paid in RLUSD on the ledger, and owes d2 no more than the
      // credit a junior extends; its owner's balance fell by what it sent.
      const after = await ledger(d1);
      assert(
        after.onLedgerBalance < before.onLedgerBalance,
        "d1 settled on the ledger",
      );
      const owes = (await peerAccounts(scenario)).get("d1>d2")!;
      assert(owes <= CREDIT, `d1 still owes ${owes}`);
      assert(after.ownerBalance <= before.ownerBalance - PAYMENT);

      // The issuer freezes d2's line: its owner sees it, as the ledger
      // says, and the issuer lifts it again.
      const { address } = (await d2.rpc.call("listLedgers", {}))
        .externalAssets[0];
      await scenario.askIssuer({
        action: "freeze",
        account: address,
        on: true,
      });
      await scenario.until(
        "the freeze shown",
        async () => (await line(d2)).holding.frozen,
        60_000,
      );
      await scenario.askIssuer({
        action: "freeze",
        account: address,
        on: false,
      });
      await scenario.until(
        "the freeze lifted",
        async () => !(await line(d2)).holding.frozen,
        60_000,
      );

      // A clawback lowers d2's line without a Payment: d2 books it as a
      // withdrawal, and its books still hold its line.
      const owned = (await ledger(d2)).ownerBalance;
      const held = (await line(d2)).units;
      await scenario.askIssuer({
        action: "clawback",
        account: address,
        value: "2",
      });
      await scenario.until(
        "the clawback booked",
        async () =>
          (await line(d2)).units === held - CLAWED && (await inStep(d2)),
        60_000,
      );
      assertEquals((await ledger(d2)).ownerBalance, owned - CLAWED);
      assertEquals(await withdrawals(d2), [CLAWED]);
      assertEquals(await withdrawals(d1), []);
    } finally {
      const failures = await scenario.stop();
      assertEquals(failures, [], failures.join("\n"));
    }
  },
);
