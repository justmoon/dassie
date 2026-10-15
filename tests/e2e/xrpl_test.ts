/**
 * The M7b done criterion (spec 7 §8): the `xrpl-settlement` scenario
 * settles and reconciles across restarts. The nodes are the real binary;
 * the XRP Ledger is the node's fake, over WebSocket, with its faucet over
 * HTTPS: CI does not depend on the public testnet. Set
 * `DASSIE_XRPL_TESTNET=1` to run it against the testnet instead, by hand.
 *
 * The faucet funds both nodes; a client of d1 pays d2's receiver several
 * times the credit, so d1 settles on the ledger as it pays; then both
 * restart, catch up, and their books match their accounts, less the
 * reserve, with nothing booked twice.
 */

import { assert, assertEquals } from "@std/assert";
import { Buffer } from "node:buffer";
// @ts-types="./types/ilp-plugin-btp.d.ts"
import BtpPlugin from "ilp-plugin-btp";
// @ts-types="./types/ws.d.ts"
import WebSocket from "ws";
import { createConnection } from "ilp-protocol-stream";
import { LedgerConnection } from "@dassie/rpc/proto";

import {
  DASSIE,
  FAKE_LEDGER,
  missingScenarioTools,
  SCENARIO_PORT_OFFSET,
} from "./harness.ts";
import { Scenario, type ScenarioNode } from "@dassie/dev/scenario";

const LEDGER = "xrpl-testnet+xrp";
/** 5 XRP: over four times what a peer extends, so d1 must settle. */
const PAYMENT = 5_000_000_000n;
/** What a node extends to each peer: 1.1 XRP. */
const CREDIT = 1_100_000_000n;

const missing = await missingScenarioTools();
const testnet = Deno.env.get("DASSIE_XRPL_TESTNET") === "1";

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

/** Each node's peer accounts: net credit, by `node>peer`. */
async function peerAccounts(scenario: Scenario): Promise<Map<string, bigint>> {
  const byId = new Map(scenario.nodes.map((node) => [node.id, node.name]));
  const accounts = new Map<string, bigint>();
  for (const node of scenario.nodes) {
    const list = await node.rpc.call("listAccounts", {});
    for (const account of list.accounts) {
      const prefix = `${LEDGER}:assets/interledger/`;
      if (!account.path.startsWith(prefix)) continue;
      const peer = byId.get(account.path.slice(prefix.length));
      accounts.set(
        `${node.name}>${peer}`,
        account.creditsPosted - account.debitsPosted,
      );
    }
  }
  return accounts;
}

/** Settling went quiet: nothing on its way, and both books agree. */
async function quiet(scenario: Scenario): Promise<boolean> {
  for (const node of scenario.nodes) {
    const { channels } = await node.rpc.call("listSettlementChannels", {});
    if (channels.some((channel) => channel.sending)) return false;
    const view = await ledger(node);
    if (view.connection !== LedgerConnection.SYNCED) return false;
  }
  const accounts = await peerAccounts(scenario);
  for (const [pair, mine] of accounts) {
    const [node, peer] = pair.split(">");
    const theirs = accounts.get(`${peer}>${node}`);
    if (theirs === undefined || mine + theirs !== 2n * CREDIT) return false;
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

Deno.test({
  name:
    "xrpl-settlement settles on the XRP Ledger, and reconciles across restarts",
  ignore: missing !== undefined,
  sanitizeOps: false,
  sanitizeResources: false,
  async fn() {
    const scenario = await Scenario.temporary("xrpl-settlement", {
      dassie: DASSIE,
      fakeLedger: testnet ? undefined : FAKE_LEDGER,
      portOffset: SCENARIO_PORT_OFFSET,
    });
    try {
      await scenario.until("funded", () => funded(scenario), 120_000);
      await scenario.until("peered", () => scenario.converged(), 120_000);
      const [d1, d2] = scenario.nodes;
      const address = (await ledger(d1)).depositAddress;
      assert(address.startsWith("r"), address);

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
      const ca = await Deno.readTextFile(scenario.caFile);
      const plugin = new BtpPlugin({ server }, {
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
      await scenario.until("settled", () => quiet(scenario), 120_000);
      // d1 paid on the ledger: its books fell by what it settled, and
      // the fees; its owner by what it sent.
      const after = await ledger(d1);
      assert(
        before.onLedgerBalance - after.onLedgerBalance >= PAYMENT - CREDIT,
        "d1 settled on the ledger",
      );
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
      await scenario.until("caught up", () => quiet(scenario), 120_000);
      assertEquals(await books(scenario), kept, "the books after a restart");
      assertEquals(await peerAccounts(scenario), accounts, "peer accounts");
    } finally {
      const failures = await scenario.stop();
      assertEquals(failures, [], failures.join("\n"));
    }
  },
});
