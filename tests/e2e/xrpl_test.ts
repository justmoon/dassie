/**
 * End to end (spec/areas/testing.md#end-to-end): the XRP Ledger under a
 * settlement hub. The `hub` scenario's nodes are the real binary; the XRP
 * Ledger is the node's fake, over WebSocket, with its faucet over HTTPS,
 * and each node's XRPL scheme follows it on proofs, as against the public
 * testnet. The scenario runs only on the fake ledger: each run founds a hub
 * of its own, whose escrow and validators must stay off the public
 * testnet, so this test has no switch to it.
 *
 * What `hub_test.ts` does not check: the faucet funds the members' owner
 * books of XRP, which follow the ledger and show an address that starts
 * with `r`; a client of one member pays the other's receiver more than
 * the payer's account in the hub can spend above its floor, so the payer
 * moves XRP into the hub as it pays, which its owner book pays; and once
 * both members restart, their owner books and hub books, their accounts'
 * counters in the hub and their peer accounts are as they were, each book
 * holding what its account on the ledger holds, so nothing was booked
 * twice.
 */

import { test } from "node:test";
import { assert, assertEquals, assertMatch } from "@std/assert";
import {
  LedgerConnection,
  type Transfer,
  TransferState,
} from "@dassie/rpc/proto";

import {
  DASSIE,
  FAKE_LEDGER,
  missingScenarioTools,
  SCENARIO_PORT_OFFSET,
} from "./harness.ts";
import {
  HUB_OWNER_LEDGER,
  HUB_WITHIN_MS,
  hubBooks,
  joinedHub,
  ledger,
  members,
  pay,
  peerAccounts,
  settledOverHub,
} from "./settling.ts";
import { HUB_LEDGER, Scenario, type ScenarioNode } from "@dassie/dev/scenario";

/** The largest packet, in internal units: 0.1 XRP. */
const MAX_PACKET = 100_000_000n;
/**
 * What the payment takes beyond what the payer's account in the hub can
 * spend above its floor: ten of the largest packets, more than the
 * trigger and the payee's redeem threshold together, so that what the
 * payer funds of its hold takes the account below its floor even with
 * what it still owes unfunded.
 */
const BEYOND_FLOOR = 10n * MAX_PACKET;
const PORT_OFFSET = SCENARIO_PORT_OFFSET + 600;

const missing = await missingScenarioTools();

/** `node`'s account in the hub, as its owner sees it: the scenario's one
 * hub. */
async function hubStatus(node: ScenarioNode) {
  const { hubs } = await node.rpc.call("getHubStatus", {});
  assertEquals(hubs.length, 1, `${node.name} is in ${hubs.length} hubs`);
  return hubs[0];
}

/** The most transfers one page of `listTransfers` holds. */
const TRANSFER_PAGE = 1_000;
/** The most pages {@linkcode transfersSince} reads: far more than a run
 * of this test writes. */
const MAX_TRANSFER_PAGES = 100;

/**
 * The transfers `node` wrote or changed after `after`, each once, as it
 * last was, by ID; and the last sequence number it read.
 */
async function transfersSince(node: ScenarioNode, after?: bigint) {
  const byId = new Map<bigint, Transfer>();
  let last = after;
  for (let page = 0; page < MAX_TRANSFER_PAGES; page++) {
    const { transfers } = await node.rpc.call("listTransfers", {
      afterSequence: last,
      limit: TRANSFER_PAGE,
    });
    for (const transfer of transfers) byId.set(transfer.id, transfer);
    if (transfers.length > 0) last = transfers[transfers.length - 1].sequence;
    if (transfers.length < TRANSFER_PAGE) return { byId, last };
  }
  throw new Error(`${node.name} has over ${MAX_TRANSFER_PAGES} pages`);
}

/** The fees `node` booked on its ledger `id` after `after`: the XRP
 * Ledger's for its transactions, or the hub's for its operations. */
async function feesSince(
  node: ScenarioNode,
  id: string,
  after: bigint | undefined,
): Promise<bigint> {
  const { byId } = await transfersSince(node, after);
  let fees = 0n;
  for (const transfer of byId.values()) {
    if (transfer.reference.case !== "fee") continue;
    if (transfer.state !== TransferState.POSTED) continue;
    if (!transfer.debitPath.startsWith(`${id}:`)) continue;
    fees += transfer.amount;
  }
  return fees;
}

/** What a member's books and its account's counters in the hub hold. */
async function kept(node: ScenarioNode) {
  const status = await hubStatus(node);
  return {
    books: await hubBooks(node),
    deposited: status.deposited,
    withdrawn: status.withdrawn,
    returned: status.returned,
  };
}

test(
  "the hub's members settle from their owner books on the XRP Ledger, and reconcile across restarts",
  { skip: missing !== undefined },
  async () => {
    const scenario = await Scenario.temporary("hub", {
      dassie: DASSIE,
      fakeLedger: FAKE_LEDGER,
      portOffset: PORT_OFFSET,
    });
    try {
      const nodes = members(scenario);
      const [payer, payee] = nodes;
      // The faucet funded each member's owner book, which follows the fake
      // ledger, and shows where the owner deposits.
      await scenario.until("owner books funded", async () => {
        for (const node of nodes) {
          const view = await ledger(node, HUB_OWNER_LEDGER);
          if (view.connection !== LedgerConnection.SYNCED) return false;
          if (view.ownerBalance <= 0n) return false;
        }
        return true;
      }, HUB_WITHIN_MS);
      for (const node of nodes) {
        const view = await ledger(node, HUB_OWNER_LEDGER);
        assert(view.faucet, `${node.name}'s testnet has a faucet`);
        assertMatch(view.depositAddress, /^r/);
      }
      await joinedHub(scenario);

      // More than the payer's account can spend above its floor, so it
      // moves XRP in from its owner book as it pays
      // (spec/areas/hubs.md#in-the-node).
      const status = await hubStatus(payer);
      assert(status.automatic, "the payer moves funds by itself");
      const spendable = status.balance > status.floor
        ? status.balance - status.floor
        : 0n;
      const payment = spendable + BEYOND_FLOOR;
      const before = await kept(payer);
      const hubBefore = await ledger(payer, HUB_LEDGER);
      const ownerBefore = await ledger(payer, HUB_OWNER_LEDGER);
      const { last: journaled } = await transfersSince(payer);
      await pay(scenario, [payer, payee], HUB_LEDGER, payment, HUB_LEDGER);
      await scenario.until(
        "settled",
        () => settledOverHub(scenario),
        HUB_WITHIN_MS,
      );
      const after = await kept(payer);
      const moved = after.deposited - before.deposited;
      assert(moved > 0n, "the payer moved nothing into the hub");
      assertEquals(after.withdrawn, before.withdrawn, "the payer withdrew");
      // The owner book paid what moved and the XRP Ledger's fees for
      // moving it; the hub book got what moved, less what was paid and
      // the hub's fees for the operations that paid it.
      const ownerAfter = await ledger(payer, HUB_OWNER_LEDGER);
      const ownerFees = await feesSince(payer, HUB_OWNER_LEDGER, journaled);
      assert(ownerFees > 0n, "the moves paid no ledger fee");
      assertEquals(
        ownerBefore.ownerBalance - ownerAfter.ownerBalance,
        moved + ownerFees,
        "what the owner book paid",
      );
      const hubAfter = await ledger(payer, HUB_LEDGER);
      const hubFees = await feesSince(payer, HUB_LEDGER, journaled);
      assertEquals(
        hubAfter.ownerBalance - hubBefore.ownerBalance,
        moved - payment - hubFees,
        "what the hub book got",
      );

      // Restarted, both members read the ledger and the hub again, and
      // book nothing twice: each book still holds what its account holds.
      const books = await Promise.all(nodes.map(kept));
      const accounts = await peerAccounts(scenario, HUB_LEDGER);
      for (const node of nodes) {
        const { code, output } = (await node.stop())!;
        assertEquals(code, 0, output);
      }
      for (const node of nodes) {
        await node.start();
        await node.waitForEngine();
      }
      await scenario.until(
        "caught up",
        () => settledOverHub(scenario),
        HUB_WITHIN_MS,
      );
      assertEquals(
        await Promise.all(nodes.map(kept)),
        books,
        "the books after a restart",
      );
      assertEquals(
        await peerAccounts(scenario, HUB_LEDGER),
        accounts,
        "peer accounts",
      );
      assertEquals(
        (await peerAccounts(scenario, HUB_OWNER_LEDGER)).size,
        0,
        "the owner books peer with nobody",
      );
    } finally {
      const failures = await scenario.stop();
      assertEquals(failures, [], failures.join("\n"));
    }
  },
);
