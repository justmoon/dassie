/**
 * End to end (spec/areas/testing.md#end-to-end): two core nodes of the
 * shipped binary swap with each other by themselves
 * (spec/decisions/peer-swaps.md). The `peer-swaps` scenario's four
 * validators found an XRP hub and a DSUSD hub on the fake ledger; d1 and
 * d2 join both, each with the faucet's XRP and a little DSUSD, and peer
 * over both hubs, a method each. The test mints each more DSUSD, d1 less
 * than its XRP is worth and d2 more, so the two are skewed opposite ways,
 * and waits for a swap between them to go through.
 *
 * That they swap until neither gains, and both end closer to their
 * targets, is checked in Rust beside daemons on paused time
 * (`crates/dassie-node/tests/peer_swaps.rs`,
 * spec/areas/testing.md#the-market-maker-and-peer-swaps-on-paused-time).
 */

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { Scenario, type ScenarioNode } from "@dassie/dev/scenario";

import {
  DASSIE,
  FAKE_LEDGER,
  missingScenarioTools,
  PORT_OFFSETS,
} from "./harness.ts";
import { freshRates } from "./market_maker.ts";
import { joinedHubs } from "./settling.ts";

/**
 * What the test mints each node beside the scenario's 25 dollars: with the
 * faucet's 1000 XRP, worth 500 dollars at the dev rates, d1 holds 48% of
 * its value in DSUSD and d2 52%.
 */
const MINT = { d1: "437", d2: "517" };
/**
 * How long their first swap may take once both are skewed: the mint
 * lands, each pushes the other a snapshot, and a swap settles over both
 * hubs. A hang's guard, not what the test decides by.
 */
const SWAP_WITHIN_MS = 180_000;
/** Clear of every other file's ports (`PORT_OFFSETS`, which ports_test.ts checks). */
const PORT_OFFSET = PORT_OFFSETS["peer_swaps_test.ts"];

const missing = await missingScenarioTools();

test(
  "two core nodes skewed opposite ways swap with each other by themselves",
  { skip: missing },
  async () => {
    const scenario = await Scenario.temporary("peer-swaps", {
      dassie: DASSIE,
      fakeLedger: FAKE_LEDGER,
      portOffset: PORT_OFFSET,
    });
    try {
      const [d1, d2] = scenario.nodes.filter((node) => node.index >= 4);
      assert(d1 && d2 && !d1.periphery && !d2.periphery, "two core nodes");
      await joinedHubs(scenario);
      for (const node of [d1, d2]) {
        await scenario.until(
          `${node.name} has fresh rates`,
          () => freshRates(node),
        );
        const { settings } = await node.rpc.call("getPeerSettings", {});
        assert(settings?.swapping && settings.offerSwaps, "swaps on");
      }
      await scenario.mintToken(d1, "DSUSD", MINT.d1);
      await scenario.mintToken(d2, "DSUSD", MINT.d2);
      // Each tries by itself, but which of them swaps is a race: either
      // one's swap closes the gap between them, and one may make every swap.
      const status = (node: ScenarioNode) => node.rpc.call("getSwapStatus", {});
      await scenario.until(
        "each tries a swap, and one goes through",
        async () => {
          const [one, two] = [await status(d1), await status(d2)];
          return one.sent > 0n && two.sent > 0n &&
            one.fulfilled + two.fulfilled > 0n;
        },
        SWAP_WITHIN_MS,
      );
      for (const node of [d1, d2]) {
        assert((await status(node)).byItself, `${node.name} swaps by itself`);
      }
    } finally {
      const failures = await scenario.stop();
      assertEquals(failures, [], failures.join("\n"));
    }
  },
);
