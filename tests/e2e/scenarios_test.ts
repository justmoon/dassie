/**
 * The M4 done criterion (spec 7 §8): the `two-nodes` and `six-nodes`
 * scenarios converge and route payments across hops; restarts keep
 * peerings and balances. Each payment is several times the credit peers
 * extend, so it only gets through if every hop settles as it goes.
 *
 * Every node is the real binary. Payments are the JavaScript reference
 * stack: `ilp-plugin-btp` connects to one node with a token it issued, and
 * `ilp-protocol-stream` pays another node's receiver through the network.
 */

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { Buffer } from "node:buffer";
import { readFile } from "node:fs/promises";
// @ts-types="./types/ilp-plugin-btp.d.ts"
import BtpPlugin from "ilp-plugin-btp";
// @ts-types="./types/ws.d.ts"
import WebSocket from "ws";
import { createConnection } from "ilp-protocol-stream";

import {
  DASSIE,
  missingScenarioTools,
  SCENARIO_PORT_OFFSET,
} from "./harness.ts";
import { Scenario, type ScenarioNode } from "@dassie/dev/scenario";

/** 5 USD: over four times what a peer extends, so every hop must settle. */
const PAYMENT = 5_000_000_000n;
/** What a node extends to each peer: 1.1 USD (spec 5 §2.7). */
const CREDIT = 1_100_000_000n;
/**
 * A node settles once it owes a peer this much net: a tenth of the credit
 * above the middle (spec 5 §4.6).
 */
const THRESHOLD = CREDIT / 10n;
const DEPOSIT = 100_000_000_000n;

const missing = await missingScenarioTools();

/** A `ws` socket class that trusts the scenario's CA. */
async function trusting(scenario: Scenario): Promise<typeof WebSocket> {
  const ca = await readFile(scenario.caFile, "utf8");
  return class extends WebSocket {
    constructor(url: string) {
      super(url, { ca });
    }
  };
}

/** A client of `from` pays `PAYMENT` to the receiver of `to`. */
async function pay(scenario: Scenario, from: ScenarioNode, to: ScenarioNode) {
  const token = await from.rpc.call("createBtpToken", {
    label: "e2e",
    ledger: "stub+usd",
  });
  const { payment: credentials } = await to.rpc.call(
    "createIncomingPayment",
    {},
  );
  assert(credentials);
  // At `localhost`, which the node's certificate names too: Deno need not
  // resolve `dN.localhost`.
  const server = token.url.replace(`@${from.hostname}:`, "@localhost:");
  const plugin = new BtpPlugin({ server }, {
    WebSocket: await trusting(scenario),
  });
  await plugin.connect();
  try {
    const connection = await createConnection({
      plugin,
      destinationAccount: credentials.destinationAccount,
      sharedSecret: Buffer.from(credentials.sharedSecret),
    });
    const stream = connection.createStream();
    await stream.sendTotal(Number(PAYMENT), { timeout: 120_000 });
    assertEquals(BigInt(stream.totalSent), PAYMENT);
    await connection.end();
  } finally {
    // The plugin would otherwise reconnect by itself.
    await plugin.disconnect();
  }
}

/** Each node's owner balance on `stub+usd`, by name. */
async function owners(scenario: Scenario): Promise<Map<string, bigint>> {
  const balances = new Map<string, bigint>();
  for (const node of scenario.nodes) {
    const [ledger] = (await node.rpc.call("listLedgers", {})).ledgers;
    assertEquals(ledger.ownerPending, 0n, `${node.name} has packets in flight`);
    balances.set(node.name, ledger.ownerBalance);
  }
  return balances;
}

/** Each node's stub balance, its "underlying ledger", by name. */
async function stubs(scenario: Scenario): Promise<Map<string, bigint>> {
  const balances = new Map<string, bigint>();
  for (const node of scenario.nodes) {
    const [ledger] = (await node.rpc.call("listLedgers", {})).ledgers;
    balances.set(node.name, ledger.onLedgerBalance);
  }
  return balances;
}

/** Each node's peer accounts: net credit, by `node>peer`. */
async function peerAccounts(scenario: Scenario): Promise<Map<string, bigint>> {
  const byId = new Map(scenario.nodes.map((node) => [node.id, node.name]));
  const accounts = new Map<string, bigint>();
  for (const node of scenario.nodes) {
    const list = await node.rpc.call("listAccounts", {});
    for (const account of list.accounts) {
      const prefix = "stub+usd:assets/interledger/";
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

/** Whether every pair of accounts mirrors the other around the credits. */
function mirrored(accounts: Map<string, bigint>): boolean {
  for (const [pair, mine] of accounts) {
    const [node, peer] = pair.split(">");
    const theirs = accounts.get(`${peer}>${node}`);
    if (theirs === undefined || mine + theirs !== 2n * CREDIT) return false;
  }
  return true;
}

function assertMirrored(accounts: Map<string, bigint>) {
  assert(mirrored(accounts), `the books disagree: ${[...accounts]}`);
}

/**
 * Whether settling has gone quiet: no settlement on its way anywhere, no
 * one owing a peer past the threshold, and every pair's books mirrored.
 */
async function quiet(scenario: Scenario): Promise<boolean> {
  for (const node of scenario.nodes) {
    const { channels } = await node.rpc.call("listSettlementChannels", {});
    if (channels.some((channel) => channel.sending)) return false;
    const { peers } = await node.rpc.call("listPeers", {});
    if (peers.some((peer) => peer.owed >= THRESHOLD)) return false;
  }
  return mirrored(await peerAccounts(scenario));
}

/**
 * The stubs' money is conserved, and moved by what was settled: what a
 * node's books say it owes its peers net is what its payments left
 * unsettled.
 */
async function assertSettled(scenario: Scenario) {
  const balances = await stubs(scenario);
  let total = 0n;
  for (const balance of balances.values()) total += balance;
  assertEquals(total, DEPOSIT * BigInt(scenario.nodes.length), "the stubs");
  const accounts = await peerAccounts(scenario);
  assertMirrored(accounts);
  const ownerBalances = await owners(scenario);
  for (const node of scenario.nodes) {
    let owed = 0n;
    for (const [pair, net] of accounts) {
      if (pair.startsWith(`${node.name}>`)) owed += net - CREDIT;
    }
    // What the owner spent net is what the stub paid plus what is owed.
    const spent = DEPOSIT - ownerBalances.get(node.name)!;
    const paid = DEPOSIT - balances.get(node.name)!;
    assertEquals(paid + owed, spent, `${node.name} settled what it spent`);
  }
}

/** Restarts every node, and checks it kept its peerings and balances. */
async function restartAll(scenario: Scenario) {
  const peerings = await scenario.peerings();
  const balances = await owners(scenario);
  const accounts = await peerAccounts(scenario);
  const settled = await stubs(scenario);
  for (const node of scenario.nodes) {
    const { code, output } = (await node.stop())!;
    assertEquals(code, 0, output);
  }
  for (const node of scenario.nodes) {
    await node.start();
    await node.waitForEngine();
  }
  assertEquals(await scenario.peerings(), peerings, "peerings after a restart");
  assertEquals(await owners(scenario), balances, "balances after a restart");
  assertEquals(await peerAccounts(scenario), accounts, "peer accounts");
  assertEquals(await stubs(scenario), settled, "stubs after a restart");
}

async function run(
  name: string,
  test: (scenario: Scenario) => Promise<void>,
) {
  const scenario = await Scenario.temporary(name, {
    dassie: DASSIE,
    portOffset: SCENARIO_PORT_OFFSET,
  });
  let passed = false;
  try {
    await test(scenario);
    passed = true;
  } finally {
    const failures = await scenario.stop();
    if (passed) assertEquals(failures, []);
  }
}

test(
  "two-nodes: the nodes peer, pay each other by settling, keep it all across restarts, and part settled",
  { skip: missing !== undefined },
  async () => {
    await run("two-nodes", async (scenario) => {
      await scenario.until("convergence", () => scenario.converged());
      const [d1, d2] = scenario.nodes;
      assertEquals(
        await scenario.peerings(),
        new Map([["d1", ["d2"]], ["d2", ["d1"]]]),
      );
      await pay(scenario, d1, d2);
      await scenario.until("settled", () => quiet(scenario));
      assertEquals(
        await owners(scenario),
        new Map([["d1", DEPOSIT - PAYMENT], ["d2", DEPOSIT + PAYMENT]]),
      );
      await assertSettled(scenario);
      await restartAll(scenario);
      await scenario.until(
        "routes after a restart",
        () => scenario.converged(),
      );
      await pay(scenario, d2, d1);
      await scenario.until("settled back", () => quiet(scenario));
      assertEquals(
        await owners(scenario),
        new Map([["d1", DEPOSIT], ["d2", DEPOSIT]]),
      );
      await assertSettled(scenario);
      // d1's owner ends the peering: it ends on both sides, each settles
      // what it still owes, and they do not peer again at once.
      await d1.rpc.call("terminatePeering", { node: d2.id });
      await scenario.until("parted", async () => {
        const peerings = await scenario.peerings();
        return peerings.get("d1")!.length === 0 &&
          peerings.get("d2")!.length === 0 && await quiet(scenario);
      });
      const accounts = await peerAccounts(scenario);
      assertEquals(accounts.get("d1>d2"), CREDIT, "d1 owes nothing");
      assertEquals(accounts.get("d2>d1"), CREDIT, "d2 owes nothing");
      await assertSettled(scenario);
      await new Promise((resolve) => setTimeout(resolve, 5_000));
      assertEquals(
        await scenario.peerings(),
        new Map([["d1", []], ["d2", []]]),
        "no new peering during the cooldown",
      );
    });
  },
);

test(
  "six-nodes: the network converges and routes a payment across hops, settling each",
  { skip: missing !== undefined },
  async () => {
    await run("six-nodes", async (scenario) => {
      await scenario.until("convergence", () => scenario.converged());
      // Two nodes that are not peers: the payment takes at least two hops.
      const peerings = await scenario.peerings();
      const [from, to] = scenario.nodes.flatMap((a) =>
        scenario.nodes
          .filter((b) => b !== a && !peerings.get(a.name)!.includes(b.name))
          .map((b) => [a, b])
      )[0];
      assert(from && to, "every node is every other's peer");
      await pay(scenario, from, to);
      await scenario.until("settled", () => quiet(scenario));
      const balances = await owners(scenario);
      for (const node of scenario.nodes) {
        const expected = node === from
          ? DEPOSIT - PAYMENT
          : node === to
          ? DEPOSIT + PAYMENT
          : DEPOSIT;
        assertEquals(balances.get(node.name), expected, node.name);
      }
      await assertSettled(scenario);
      const settled = await stubs(scenario);
      assert(
        settled.get(from.name)! <= DEPOSIT - (PAYMENT - CREDIT),
        "the payer settled",
      );
      await restartAll(scenario);
    });
  },
);
