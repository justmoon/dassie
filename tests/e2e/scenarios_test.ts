/**
 * End to end (spec/areas/testing.md#end-to-end): the `two-nodes` and
 * `six-nodes` scenarios converge and route payments across hops; restarts
 * keep peerings and balances. Their nodes run the stub with unlimited
 * trust (spec/areas/settlement.md#the-stub-scheme): peers on it never run
 * out of room and settle nothing, so each payment, several times the
 * default credit, gets through without a settlement, the stubs never
 * move, and each node's books owe its peers what its owner spent.
 *
 * Every node is the real binary. Payments are the JavaScript reference
 * stack: `ilp-plugin-btp` connects to one node with a token it issued, and
 * `ilp-protocol-stream` pays another node's receiver through the network.
 */

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { Buffer } from "node:buffer";
import { readFile } from "node:fs/promises";
import WebSocket from "#ws";
import { createConnection } from "ilp-protocol-stream";

import { DASSIE, missingScenarioTools, PORT_OFFSETS } from "./harness.ts";
import { BtpClient } from "./btp.ts";
import { Scenario, type ScenarioNode } from "@dassie/dev/scenario";

/** 5 USD: over four times the default credit line, which the stub with
 * unlimited trust goes far past. */
const PAYMENT = 5_000_000_000n;
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

/**
 * A client of `from` pays `PAYMENT` to the receiver of `to`: on the stub
 * with unlimited trust, every peer extends the other the unlimited credit,
 * so nothing is settled first.
 */
async function pay(scenario: Scenario, from: ScenarioNode, to: ScenarioNode) {
  await scenario.until("quiet", () => quiet(scenario));
  const token = await from.rpc.call("createBtpToken", {
    label: "e2e",
    ledger: "stub+usd",
  });
  const { payment: credentials } = await to.rpc.call(
    "createIncomingPayment",
    {},
  );
  assert(credentials);
  // At `localhost`, which the node's certificate names too: Node need not
  // resolve `dN.localhost`.
  const server = token.url.replace(`@${from.hostname}:`, "@localhost:");
  const plugin = new BtpClient({ server }, {
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
      const match = /^stub\+usd:(assets\/interledger|contra\/trust)\/(.+)$/
        .exec(account.path);
      if (!match) continue;
      const key = `${node.name}>${byId.get(match[2])}`;
      const net = account.creditsPosted - account.debitsPosted;
      accounts.set(key, (accounts.get(key) ?? 0n) + net);
    }
  }
  return accounts;
}

/** Whether every pair of books mirrors the other: what one owes, the other
 * is owed. */
function mirrored(accounts: Map<string, bigint>): boolean {
  for (const [pair, mine] of accounts) {
    const [node, peer] = pair.split(">");
    const theirs = accounts.get(`${peer}>${node}`);
    if (theirs === undefined || mine + theirs !== 0n) return false;
  }
  return true;
}

function assertMirrored(accounts: Map<string, bigint>) {
  assert(mirrored(accounts), `the books disagree: ${[...accounts]}`);
}

/**
 * Whether the network is quiet: no settlement on its way anywhere, no
 * packet in flight, and every pair's books mirrored.
 */
async function quiet(scenario: Scenario): Promise<boolean> {
  for (const node of scenario.nodes) {
    const { channels } = await node.rpc.call("listSettlementChannels", {});
    if (channels.some((channel) => channel.sending)) return false;
    const [ledger] = (await node.rpc.call("listLedgers", {})).ledgers;
    if (ledger.ownerPending !== 0n) return false;
  }
  return mirrored(await peerAccounts(scenario));
}

/**
 * Nothing was settled: every stub holds what it started with, and what a
 * node's books say it owes its peers net is what its owner spent.
 */
async function assertNothingSettled(scenario: Scenario) {
  const balances = await stubs(scenario);
  for (const node of scenario.nodes) {
    assertEquals(balances.get(node.name), DEPOSIT, `${node.name}'s stub`);
  }
  const accounts = await peerAccounts(scenario);
  assertMirrored(accounts);
  const ownerBalances = await owners(scenario);
  for (const node of scenario.nodes) {
    let owed = 0n;
    for (const [pair, net] of accounts) {
      if (pair.startsWith(`${node.name}>`)) owed += net;
    }
    const spent = DEPOSIT - ownerBalances.get(node.name)!;
    assertEquals(owed, spent, `${node.name} owes what it spent`);
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
    portOffset: PORT_OFFSETS["scenarios_test.ts"],
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
  "two-nodes: the nodes peer, pay each other settling nothing, keep it all across restarts, and part",
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
      await scenario.until("quiet", () => quiet(scenario));
      assertEquals(
        await owners(scenario),
        new Map([["d1", DEPOSIT - PAYMENT], ["d2", DEPOSIT + PAYMENT]]),
      );
      await assertNothingSettled(scenario);
      await restartAll(scenario);
      await scenario.until(
        "routes after a restart",
        () => scenario.converged(),
      );
      await pay(scenario, d2, d1);
      await scenario.until("quiet again", () => quiet(scenario));
      assertEquals(
        await owners(scenario),
        new Map([["d1", DEPOSIT], ["d2", DEPOSIT]]),
      );
      await assertNothingSettled(scenario);
      // d1's owner ends the peering: d1 asks d2 to settle once more, which
      // on the stub with unlimited trust settles nothing, then tells it,
      // and it ends on both sides (spec/decisions/ending-a-peering.md).
      // Nothing moves, and they do not peer again at once.
      const before = await peerAccounts(scenario);
      await d1.rpc.call("terminatePeering", { node: d2.id });
      await scenario.until("parted", async () => {
        const peerings = await scenario.peerings();
        return peerings.get("d1")!.length === 0 &&
          peerings.get("d2")!.length === 0 && await quiet(scenario);
      });
      assertEquals(await peerAccounts(scenario), before, "nothing moved");
      assertEquals(
        await owners(scenario),
        new Map([["d1", DEPOSIT], ["d2", DEPOSIT]]),
      );
      await assertNothingSettled(scenario);
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
  "six-nodes: the network converges and routes a payment across hops, settling nothing",
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
      await scenario.until("quiet", () => quiet(scenario));
      const balances = await owners(scenario);
      for (const node of scenario.nodes) {
        const expected = node === from
          ? DEPOSIT - PAYMENT
          : node === to
          ? DEPOSIT + PAYMENT
          : DEPOSIT;
        assertEquals(balances.get(node.name), expected, node.name);
      }
      await assertNothingSettled(scenario);
      await restartAll(scenario);
    });
  },
);
