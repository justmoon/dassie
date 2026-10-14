/**
 * The M4a done criterion (spec 7 §8): the `two-nodes` and `six-nodes`
 * scenarios converge and route payments across hops, within the credit
 * peers extend; restarts keep peerings and balances.
 *
 * Every node is the real binary. Payments are the JavaScript reference
 * stack: `ilp-plugin-btp` connects to one node with a token it issued, and
 * `ilp-protocol-stream` pays another node's receiver through the network.
 */

import { assert, assertEquals } from "@std/assert";
import { Buffer } from "node:buffer";
// @ts-types="./types/ilp-plugin-btp.d.ts"
import BtpPlugin from "ilp-plugin-btp";
// @ts-types="./types/ws.d.ts"
import WebSocket from "ws";
import { createConnection } from "ilp-protocol-stream";

import { missingBinary } from "./harness.ts";
import {
  ROOT,
  Scenario,
  type ScenarioName,
  type ScenarioNode,
} from "./scenarios.ts";

/** A quarter of a USD: several packets, within one peer's credit. */
const PAYMENT = 250_000_000n;
/** What a node extends to each peer: 1.1 USD (spec 5 §2.7). */
const CREDIT = 1_100_000_000n;
const DEPOSIT = 100_000_000_000n;

const missing = await missingBinary();

/** A `ws` socket that trusts the test root. */
class TrustingWebSocket extends WebSocket {
  constructor(url: string) {
    super(url, { ca: ROOT });
  }
}

/** A client of `from` pays `PAYMENT` to the receiver of `to`. */
async function pay(from: ScenarioNode, to: ScenarioNode) {
  const token = await from.rpc.call("createBtpToken", {
    label: "e2e",
    ledger: "stub+usd",
  });
  const credentials = await to.rpc.call("createStreamCredentials", {
    tag: "e2e",
  });
  const plugin = new BtpPlugin({ server: token.url }, {
    WebSocket: TrustingWebSocket,
  });
  await plugin.connect();
  try {
    const connection = await createConnection({
      plugin,
      destinationAccount: credentials.destinationAccount,
      sharedSecret: Buffer.from(credentials.sharedSecret),
    });
    const stream = connection.createStream();
    await stream.sendTotal(Number(PAYMENT));
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

/** Each peering's two accounts mirror each other around the two credits. */
function assertMirrored(accounts: Map<string, bigint>) {
  for (const [pair, mine] of accounts) {
    const [node, peer] = pair.split(">");
    const theirs = accounts.get(`${peer}>${node}`);
    assert(theirs !== undefined, `${peer} has no account for ${node}`);
    assertEquals(mine + theirs, 2n * CREDIT, `${pair} and back disagree`);
  }
}

/** Restarts every node, and checks it kept its peerings and balances. */
async function restartAll(scenario: Scenario) {
  const peerings = await scenario.peerings();
  const balances = await owners(scenario);
  const accounts = await peerAccounts(scenario);
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
}

async function run(
  name: ScenarioName,
  test: (scenario: Scenario) => Promise<void>,
) {
  const scenario = await Scenario.start(name);
  let passed = false;
  try {
    await test(scenario);
    passed = true;
  } finally {
    const failures = await scenario.stop();
    if (passed) assertEquals(failures, []);
  }
}

Deno.test({
  name:
    "two-nodes: the nodes peer, pay each other, and keep it all across restarts",
  ignore: missing !== undefined,
  sanitizeResources: false,
  sanitizeOps: false,
  async fn() {
    await run("two-nodes", async (scenario) => {
      await scenario.until("convergence", () => scenario.converged());
      const [d0, d1] = scenario.nodes;
      assertEquals(
        await scenario.peerings(),
        new Map([["d0", ["d1"]], ["d1", ["d0"]]]),
      );
      await pay(d0, d1);
      assertEquals(
        await owners(scenario),
        new Map([["d0", DEPOSIT - PAYMENT], ["d1", DEPOSIT + PAYMENT]]),
      );
      const accounts = await peerAccounts(scenario);
      assertMirrored(accounts);
      assertEquals(accounts.get("d0>d1"), CREDIT + PAYMENT);
      await restartAll(scenario);
      await scenario.until(
        "routes after a restart",
        () => scenario.converged(),
      );
      await pay(d1, d0);
      assertEquals(
        await owners(scenario),
        new Map([["d0", DEPOSIT], ["d1", DEPOSIT]]),
      );
    });
  },
});

Deno.test({
  name: "six-nodes: the network converges and routes a payment across hops",
  ignore: missing !== undefined,
  sanitizeResources: false,
  sanitizeOps: false,
  async fn() {
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
      await pay(from, to);
      const balances = await owners(scenario);
      for (const node of scenario.nodes) {
        const expected = node === from
          ? DEPOSIT - PAYMENT
          : node === to
          ? DEPOSIT + PAYMENT
          : DEPOSIT;
        assertEquals(balances.get(node.name), expected, node.name);
      }
      assertMirrored(await peerAccounts(scenario));
      await restartAll(scenario);
    });
  },
});
