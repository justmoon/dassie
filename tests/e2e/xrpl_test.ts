/**
 * End to end (spec/areas/testing.md#end-to-end): the `xrpl-settlement`
 * scenario settles and reconciles across restarts. The nodes are the real
 * binary; the XRP Ledger is the node's fake, over WebSocket, with its
 * faucet over HTTPS: CI does not depend on the public testnet. Set
 * `DASSIE_XRPL_TESTNET=1` to run it against the testnet instead, by hand.
 *
 * The faucet funds both nodes; a client of d1 pays d2's receiver several
 * times the credit, so d1 settles on the ledger as it pays; then both
 * restart, catch up, and their books match their accounts, less the
 * reserve, with nothing booked twice.
 */

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { Buffer } from "node:buffer";
import { readFile } from "node:fs/promises";
import process from "node:process";
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

const LEDGER = "xrpl-testnet-xrp+xrp";
/** 5 XRP: over four times what a peer extends, so d1 must settle. */
const PAYMENT = 5_000_000_000n;
/** What a node extends to each peer: 1.1 XRP. */
const CREDIT = 1_100_000_000n;
/** Every node's trigger: one and a half of the largest packets (spec/decisions/thresholds-in-packets.md). */
const TRIGGER = 150_000_000n;
const PORT_OFFSET = SCENARIO_PORT_OFFSET + 600;

const missing = await missingScenarioTools();
const testnet = process.env.DASSIE_XRPL_TESTNET === "1";

/** A node's XRPL ledger, as its owner sees it. */
async function ledger(node: ScenarioNode) {
  const { ledgers } = await node.rpc.call("listLedgers", {});
  const found = ledgers.find((l) => l.id === LEDGER);
  assert(found, `${node.name} has no ${LEDGER}`);
  return found;
}

/** Whether every node follows the ledger, and has been funded. */
async function funded(scenario: Scenario): Promise<boolean> {
  for (const node of scenario.nodes) {
    const view = await ledger(node);
    if (view.connection !== LedgerConnection.SYNCED) return false;
    if (view.ownerBalance <= 0n) return false;
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
 * Settling went quiet: nothing on its way, no node's books showing a peer
 * at its trigger (its room for the peer down to one and a half of the
 * largest packets, spec/decisions/thresholds-in-packets.md), and both books
 * agree. A senior extends its junior nothing, so until the junior paid in
 * advance it is at its trigger.
 */
async function quiet(scenario: Scenario): Promise<boolean> {
  for (const node of scenario.nodes) {
    const { channels } = await node.rpc.call("listSettlementChannels", {});
    if (channels.some((channel) => channel.sending)) return false;
    const view = await ledger(node);
    if (view.connection !== LedgerConnection.SYNCED) return false;
    const { peers } = await node.rpc.call("listPeers", {});
    if (peers.some((peer) => peer.creditOut + peer.owed <= TRIGGER)) {
      return false;
    }
  }
  const accounts = await peerAccounts(scenario);
  for (const [pair, mine] of accounts) {
    const [node, peer] = pair.split(">");
    const theirs = accounts.get(`${peer}>${node}`);
    if (theirs === undefined || mine + theirs !== 0n) return false;
  }
  return accounts.size > 0;
}

/** What the books hold, and what the owners have, by node. */
async function books(scenario: Scenario) {
  const out = new Map<string, [bigint, bigint]>();
  for (const node of scenario.nodes) {
    const view = await ledger(node);
    out.set(node.name, [view.onLedgerBalance, view.ownerBalance]);
  }
  return out;
}

/**
 * Longer than a node's settlement interval (4 s, 1 s with dev timings): a
 * settlement that is due starts within it.
 */
const SETTLE_CHECK_MS = 5_000;

/**
 * Settling is over: quiet, and still quiet with the same books once a
 * settlement interval has passed. Quiet alone also holds between two
 * settlements, while the next one is due but not yet started.
 */
async function settled(scenario: Scenario): Promise<boolean> {
  if (!(await quiet(scenario))) return false;
  const first = await books(scenario);
  await new Promise((resolve) => setTimeout(resolve, SETTLE_CHECK_MS));
  if (!(await quiet(scenario))) return false;
  const second = await books(scenario);
  return [...first].every(([name, [onLedger, owner]]) => {
    const now = second.get(name);
    return now !== undefined && now[0] === onLedger && now[1] === owner;
  });
}

test(
  "xrpl-settlement settles on the XRP Ledger, and reconciles across restarts",
  { skip: missing !== undefined },
  async () => {
    const scenario = await Scenario.temporary("xrpl-settlement", {
      dassie: DASSIE,
      fakeLedger: testnet ? undefined : FAKE_LEDGER,
      portOffset: PORT_OFFSET,
    });
    try {
      await scenario.until("funded", () => funded(scenario), 120_000);
      await scenario.until("peered", () => scenario.converged(), 120_000);
      const [d1, d2] = scenario.nodes;
      const address = (await ledger(d1)).depositAddress;
      assert(address.startsWith("r"), address);
      // The junior pays its senior in advance before it can send through
      // it, on the ledger (spec/decisions/earned-trust.md).
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
      // d1 paid on the ledger, and owes d2 no more than the credit a
      // junior extends; its owner's balance fell by what it sent.
      const after = await ledger(d1);
      assert(
        after.onLedgerBalance < before.onLedgerBalance,
        "d1 settled on the ledger",
      );
      const owes = (await peerAccounts(scenario)).get("d1>d2")!;
      assert(owes <= CREDIT, `d1 still owes ${owes}`);
      assertEquals(after.ownerBalance <= before.ownerBalance - PAYMENT, true);

      // Restarted, both catch up and book nothing twice.
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
      await scenario.until("caught up", () => settled(scenario), 120_000);
      assertEquals(await books(scenario), kept, "the books after a restart");
      assertEquals(await peerAccounts(scenario), accounts, "peer accounts");
    } finally {
      const failures = await scenario.stop();
      assertEquals(failures, [], failures.join("\n"));
    }
  },
);
