/**
 * What the end-to-end tests of mainnet's schemes in the live realm share
 * (`mainnet_test.ts`, `mainnet_xrp_test.ts`), and of a settlement hub
 * (`hub_test.ts`): a node's ledger, whether it
 * runs in the live realm, what it owes its peers by its own books, when
 * settling is over, and a payment from a BTP client of one node to
 * another's receiver.
 */

import { assert, assertEquals } from "@std/assert";
import { Buffer } from "node:buffer";
import { readFile } from "node:fs/promises";
import WebSocket from "#ws";
import { createConnection } from "ilp-protocol-stream";
import { LedgerConnection, Realm } from "@dassie/rpc/proto";

import { BtpClient } from "./btp.ts";
import type { Scenario, ScenarioNode } from "@dassie/dev/scenario";

/** Every node's trigger: one and a half of the largest packets
 * (spec/decisions/thresholds-in-packets.md). */
export const TRIGGER = 150_000_000n;
/** What a node extends to each peer. */
export const CREDIT = 1_100_000_000n;
/** Longer than a node's settlement interval with dev timings. */
const SETTLE_CHECK_MS = 5_000;

/** Node `node`'s ledger `id`, as its owner sees it. */
export async function ledger(node: ScenarioNode, id: string) {
  const { ledgers } = await node.rpc.call("listLedgers", {});
  const found = ledgers.find((l) => l.id === id);
  assert(found, `${node.name} has no ${id}`);
  return found;
}

/**
 * Every node runs in the live realm: its ledgers are of it, the schemes it
 * offers are mainnet's alone, and an incoming payment's address is under
 * `g.`.
 */
export async function assertLive(
  node: ScenarioNode,
  id: string,
): Promise<void> {
  assertEquals((await ledger(node, id)).realm, Realm.LIVE);
  const { schemes } = await node.rpc.call("listSettlementSchemes", {});
  assert(schemes.length > 0, `${node.name} offers no scheme`);
  for (const scheme of schemes) {
    assertEquals(scheme.realm, Realm.LIVE, `${node.name} offers ${scheme.id}`);
  }
  assertEquals(
    schemes.map((scheme) => scheme.id).sort(),
    ["xrpl-rlusd", "xrpl-xrp"],
  );
  const { payment } = await node.rpc.call("createIncomingPayment", {});
  assert(
    payment?.destinationAccount.startsWith("g."),
    payment?.destinationAccount,
  );
}

/**
 * What each node owes each peer net on ledger `id`, by its own books, by
 * `node>peer`: its peer account less the credit it extends, negative when
 * the peer owes it.
 */
export async function peerAccounts(
  scenario: Scenario,
  id: string,
): Promise<Map<string, bigint>> {
  const byId = new Map(scenario.nodes.map((node) => [node.id, node.name]));
  const accounts = new Map<string, bigint>();
  for (const node of scenario.nodes) {
    const list = await node.rpc.call("listAccounts", {});
    for (const account of list.accounts) {
      for (const kind of ["assets/interledger", "contra/trust"]) {
        const prefix = `${id}:${kind}/`;
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
 * Settling on ledger `id` went quiet: nothing on its way, every node synced
 * and, by `inStep`, in step with its account, and no node's books showing
 * a peer at its trigger.
 */
async function quiet(
  scenario: Scenario,
  id: string,
  inStep: (node: ScenarioNode) => Promise<boolean>,
): Promise<boolean> {
  for (const node of scenario.nodes) {
    const { channels } = await node.rpc.call("listSettlementChannels", {});
    if (channels.some((channel) => channel.sending)) return false;
    if ((await ledger(node, id)).connection !== LedgerConnection.SYNCED) {
      return false;
    }
    if (!(await inStep(node))) return false;
    const { peers } = await node.rpc.call("listPeers", {});
    if (peers.some((peer) => peer.creditOut + peer.owed <= TRIGGER)) {
      return false;
    }
  }
  return (await peerAccounts(scenario, id)).size > 0;
}

/**
 * Settling on ledger `id` is over: quiet, and still quiet with the same
 * books once a settlement interval has passed.
 */
export async function settled(
  scenario: Scenario,
  id: string,
  inStep: (node: ScenarioNode) => Promise<boolean> = () =>
    Promise.resolve(true),
): Promise<boolean> {
  if (!(await quiet(scenario, id, inStep))) return false;
  const views = () => Promise.all(scenario.nodes.map((n) => ledger(n, id)));
  const first = await views();
  await new Promise((resolve) => setTimeout(resolve, SETTLE_CHECK_MS));
  if (!(await quiet(scenario, id, inStep))) return false;
  const second = await views();
  return first.every((view, index) =>
    view.onLedgerBalance === second[index].onLedgerBalance &&
    view.ownerBalance === second[index].ownerBalance
  );
}

/**
 * A client of `from` with a BTP token on ledger `id` pays `amount` to an
 * incoming payment of `to`'s on ledger `receive`, its default ledger if
 * empty, over STREAM; the ILP address it paid to.
 */
export async function pay(
  scenario: Scenario,
  [from, to]: [ScenarioNode, ScenarioNode],
  id: string,
  amount: bigint,
  receive = "",
): Promise<string> {
  const token = await from.rpc.call("createBtpToken", {
    label: "e2e",
    ledger: id,
  });
  const { payment: credentials } = await to.rpc.call(
    "createIncomingPayment",
    { ledger: receive },
  );
  assert(credentials);
  const server = token.url.replace(`@${from.hostname}:`, "@localhost:");
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
    await stream.sendTotal(Number(amount), { timeout: 240_000 });
    assertEquals(BigInt(stream.totalSent), amount);
    await connection.end();
  } finally {
    await plugin.disconnect();
  }
  return credentials.destinationAccount;
}
