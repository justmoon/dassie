/**
 * End to end (spec/areas/testing.md#end-to-end): mainnet's XRP scheme in
 * the live realm (spec/decisions/mainnet-schemes.md), on the fake ledger's
 * mainnet, never real money. The nodes are the real binary, built with
 * debug assertions, set to the live realm through the dev switch
 * (`DASSIE_DEV_LIVE_REALM`) before they are claimed; a release binary
 * refuses that switch, which the release checks test.
 *
 * Mainnet has no faucet, so each node's account is funded by a deposit, as
 * its owner would fund it. On `xrpl-live-settlement`, a client of d1 pays
 * d2's receiver several times the credit, so d1 settles on the ledger as it
 * pays, at addresses under `g.`. RLUSD's scheme is `mainnet_test.ts`'s.
 */

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { LedgerConnection } from "@dassie/rpc/proto";

import { DASSIE, FAKE_LEDGER, missingScenarioTools } from "./harness.ts";
import {
  assertLive,
  CREDIT,
  ledger,
  pay,
  peerAccounts,
  settled,
} from "./settling.ts";
import { Scenario } from "@dassie/dev/scenario";

const XRP_LEDGER = "xrpl+xrp";
/** 5 XRP: over four times what a peer extends, so d1 must settle. */
const PAYMENT = 5_000_000_000n;
/**
 * Clear of every other file's ports, which run beside these, as
 * market_maker_targets_test.ts lays them out: at 12000 this file's nodes
 * take 16001 up and 17001 up on TCP, and 18001 up for their feeds on UDP,
 * and its fake ledger 22200 and 22201.
 */
const PORT_OFFSET = 12_000;

const missing = await missingScenarioTools();

test(
  "xrpl-live-settlement settles in XRP on the fake mainnet, in the live realm",
  { skip: missing !== undefined },
  async () => {
    const scenario = await Scenario.temporary("xrpl-live-settlement", {
      dassie: DASSIE,
      fakeLedger: FAKE_LEDGER,
      portOffset: PORT_OFFSET,
    });
    try {
      const funded = async () => {
        for (const node of scenario.nodes) {
          const view = await ledger(node, XRP_LEDGER);
          if (view.connection !== LedgerConnection.SYNCED) return false;
          if (view.ownerBalance <= 0n) return false;
        }
        return true;
      };
      await scenario.until("funded by a deposit", funded, 120_000);
      await scenario.until("peered", () => scenario.converged(), 120_000);
      const [d1, d2] = scenario.nodes;
      for (const node of scenario.nodes) {
        await assertLive(node, XRP_LEDGER);
        // Mainnet has no faucet.
        assertEquals((await ledger(node, XRP_LEDGER)).faucet, false);
        const { externalAssets } = await node.rpc.call("listLedgers", {});
        assertEquals(externalAssets.map((asset) => asset.id), [
          "xrpl-account/0",
        ]);
      }
      await scenario.until(
        "paid in advance",
        () => settled(scenario, XRP_LEDGER),
        120_000,
      );

      const before = await ledger(d1, XRP_LEDGER);
      const address = await pay(scenario, [d1, d2], XRP_LEDGER, PAYMENT);
      assert(address.startsWith("g."), address);
      await scenario.until(
        "settled",
        () => settled(scenario, XRP_LEDGER),
        120_000,
      );
      // d1 paid in XRP on the ledger, and owes d2 no more than the credit
      // a junior extends.
      const after = await ledger(d1, XRP_LEDGER);
      assert(
        after.onLedgerBalance < before.onLedgerBalance,
        "d1 settled on the ledger",
      );
      const owes = (await peerAccounts(scenario, XRP_LEDGER)).get("d1>d2")!;
      assert(owes <= CREDIT, `d1 still owes ${owes}`);
      assert(after.ownerBalance <= before.ownerBalance - PAYMENT);
    } finally {
      const failures = await scenario.stop();
      assertEquals(failures, [], failures.join("\n"));
    }
  },
);
