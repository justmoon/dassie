/**
 * End to end (spec/areas/testing.md#end-to-end): two core nodes swap with
 * each other (spec/decisions/peer-swaps.md). The `peer-swaps` scenario's
 * four validators found an XRP hub and a DSUSD hub on the fake ledger; d1
 * and d2 join both, each with the faucet's XRP and a little DSUSD, and
 * peer over both hubs, a method each.
 *
 * Each owner sets a spread, and the test mints each more DSUSD, d1 less
 * than its XRP is worth and d2 more, so the two are skewed opposite ways:
 * d1 short of DSUSD, d2 short of XRP. Each reads the other's snapshot, and
 * swaps with it by itself while the other's quotes, valued at its own
 * curve, clear its costs: d1 pays XRP and is paid back DSUSD, d2 the other
 * way. They swap until neither's quotes clear the other's costs, and both
 * end closer to their targets, an even split of their value.
 */

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { DEV_RATES, Scenario, type ScenarioNode } from "@dassie/dev/scenario";
import type { PeerSnapshot } from "@dassie/rpc/proto";

import {
  DASSIE,
  FAKE_LEDGER,
  missingScenarioTools,
  PORT_OFFSETS,
} from "./harness.ts";
import {
  DSUSD,
  DSUSD_METHOD,
  freshRates,
  gainPpm,
  held,
  peerSnapshot,
  UNIT,
  XRP,
  XRP_METHOD,
} from "./market_maker.ts";
import { HUB_WITHIN_MS, joinedHubs, settledOverHubs } from "./settling.ts";

/**
 * What the test mints each node beside the scenario's 25 dollars: with the
 * faucet's 1000 XRP, worth 500 dollars at the dev rates, d1 holds 48% of
 * its value in DSUSD and d2 52%.
 */
const MINT = { d1: 437n, d2: 517n };
/**
 * Each owner's spread, in parts per million: 0.5%. The curves of nodes so
 * skewed differ by about 0.8% at the start, so each swap gains a little
 * beyond the spread, and they stop once the curves are within it: after a
 * few dollars, some score of the largest packets each way, rather than
 * once their curves meet.
 */
const SPREAD_PPM = 5_000;
/**
 * The most either gains, in parts per million, on a swap at the top of
 * the other's book once they stopped. A node swaps while what the other's
 * snapshot promises, valued at its own curve, clears one plus its costs
 * (spec/areas/connector.md#the-swap-service); the top of the book promises
 * a little more than any step of the walk, the grid's first drop on each
 * ladder, and a node's own bids, spread and all, value the two assets as
 * its curve does, the spread folded into both. So once they stopped, the
 * gain here is at most a node's costs over these hubs, a fee per largest
 * packet, and those two drops: well below this, and this below the gain
 * the skew starts them at.
 */
const QUIET_PPM = 2_000n;
/** How long they may take to swap until neither gains. */
const RUN_MS = 480_000;
/** Clear of every other file's ports (`PORT_OFFSETS`, which ports_test.ts checks). */
const PORT_OFFSET = PORT_OFFSETS["peer_swaps_test.ts"];

const missing = await missingScenarioTools();

/** What a node holds of DSUSD and of XRP, in dollars at the dev rates, in
 * the books' units. */
async function dollarsOf(node: ScenarioNode): Promise<[bigint, bigint]> {
  // DEV_RATES.XRP is how many XRP a dollar buys.
  const xrp = await held(node, XRP) / BigInt(DEV_RATES.XRP);
  return [await held(node, DSUSD), xrp];
}

/** How far a node is from its targets, an even split of its value: the
 * gap between its DSUSD and its XRP, and their sum. */
async function skew(node: ScenarioNode): Promise<[bigint, bigint]> {
  const [dsusd, xrp] = await dollarsOf(node);
  const gap = dsusd > xrp ? dsusd - xrp : xrp - dsusd;
  return [gap, dsusd + xrp];
}

test(
  "two core nodes skewed opposite ways swap until neither gains, and both end closer to their targets",
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
        // SetPeerSettings sets every setting: the others stay as they are.
        const { settings } = await node.rpc.call("getPeerSettings", {});
        assert(settings?.swapping && settings.offerSwaps, "swaps on");
        await node.rpc.call("setPeerSettings", {
          settings: { ...settings, fxSpreadPpm: SPREAD_PPM },
        });
      }
      const dsusdBefore = [await held(d1, DSUSD), await held(d2, DSUSD)];
      await scenario.mintToken(d1, "DSUSD", `${MINT.d1}`);
      await scenario.mintToken(d2, "DSUSD", `${MINT.d2}`);
      // The mint reached each owner book: DSUSD is all but the whole of
      // what was minted up from before, the hub's fees on the node's own
      // moves taken.
      await scenario.until("the DSUSD minted", async () => {
        const [one, two] = [await held(d1, DSUSD), await held(d2, DSUSD)];
        return one > dsusdBefore[0]! + (MINT.d1 - 1n) * UNIT &&
          two > dsusdBefore[1]! + (MINT.d2 - 1n) * UNIT;
      }, HUB_WITHIN_MS);
      const before = [await skew(d1), await skew(d2)];
      // Skewed opposite ways: d1 holds less DSUSD than XRP, d2 more.
      const [[d1Dsusd, d1Xrp], [d2Dsusd, d2Xrp]] = [
        await dollarsOf(d1),
        await dollarsOf(d2),
      ];
      assert(d1Dsusd < d1Xrp, "d1 short of DSUSD");
      assert(d2Dsusd > d2Xrp, "d2 short of XRP");

      // Each node's gain on each way of swapping with the other, by their
      // snapshots as each holds the other's: undefined where a side is
      // closed, as while a rate the node converts at is stale.
      const gains = (ofD1: PeerSnapshot, ofD2: PeerSnapshot) => ({
        d1PaysXrp: gainPpm(ofD1, ofD2, XRP_METHOD, DSUSD_METHOD),
        d1PaysDsusd: gainPpm(ofD1, ofD2, DSUSD_METHOD, XRP_METHOD),
        d2PaysXrp: gainPpm(ofD2, ofD1, XRP_METHOD, DSUSD_METHOD),
        d2PaysDsusd: gainPpm(ofD2, ofD1, DSUSD_METHOD, XRP_METHOD),
      });
      const status = (node: ScenarioNode) => node.rpc.call("getSwapStatus", {});
      // They stopped once neither sent a swap since a moment when nothing
      // was pending, and each has since been pushed a snapshot of the
      // other made after that moment: each decided again on the other's
      // quotes as they are after every swap, and sent nothing. Pending is
      // a swap in flight, a peer backed off from (T04, T05 and the like),
      // or one waiting for its next snapshot after a refusal or a swap that
      // came back short: each of those only defers the next decision, and
      // under load a backoff grows to half a minute. And both snapshots
      // price every way, so that they decided on open quotes: a side closed
      // for a stale rate only pauses them too. A node looks at a new
      // snapshot in the step that takes it, which the owner's calls wait
      // for.
      let calm: { sent: [bigint, bigint]; at: number } | undefined;
      let settled: ReturnType<typeof gains> | undefined;
      let last: unknown;
      await scenario.until(
        "they swap until neither's quotes clear the other's costs",
        async () => {
          const at = Date.now();
          const [one, two] = [await status(d1), await status(d2)];
          const [ofD2, ofD1] = [
            await peerSnapshot(d1, d2),
            await peerSnapshot(d2, d1),
          ];
          const now = ofD1 && ofD2 ? gains(ofD1, ofD2) : undefined;
          last = { now, d1: one, d2: two, calm };
          const sent: [bigint, bigint] = [one.sent, two.sent];
          const idle = [one, two].every((swaps) =>
            swaps.inFlight === 0n && swaps.backingOff === 0n &&
            swaps.waitingForSnapshot === 0n
          );
          if (!idle || !calm || calm.sent.some((n, i) => n !== sent[i])) {
            calm = idle ? { sent, at } : undefined;
            return false;
          }
          const since = BigInt(calm.at);
          const fresh = [ofD2, ofD1].every((snapshot) =>
            snapshot !== undefined && snapshot.sequence > since
          );
          const open = now !== undefined &&
            Object.values(now).every((gain) => gain !== undefined);
          if (!fresh || !open || one.fulfilled === 0n || two.fulfilled === 0n) {
            return false;
          }
          settled = now;
          return true;
        },
        RUN_MS,
      ).catch((error) => {
        throw new Error(
          `${(error as Error).message}: ${
            JSON.stringify(last, (_, v) => typeof v === "bigint" ? `${v}` : v)
          }`,
        );
      });
      for (const node of [d1, d2]) {
        assert((await status(node)).byItself, `${node.name} swaps by itself`);
      }
      // Neither's quotes clear the other's costs: at the top of the other's
      // book, valued at the swapping node's own curve, no swap gains more
      // than its costs, well below the spread they started beyond.
      assert(settled, "the gains they settled at");
      for (const [way, gain] of Object.entries(settled)) {
        assert(
          gain !== undefined && gain < QUIET_PPM,
          `${way} gains ${gain} ppm`,
        );
      }
      await scenario.until(
        "settled",
        () => settledOverHubs(scenario),
        HUB_WITHIN_MS,
      );
      // Each is closer to an even split of its value: the gap between its
      // DSUSD and its XRP, as a share of the whole, fell.
      for (const [index, node] of [d1, d2].entries()) {
        const [gapBefore, totalBefore] = before[index]!;
        const [gap, total] = await skew(node);
        assert(
          gap * totalBefore < gapBefore * total,
          `${node.name} went from a gap of ${gapBefore} in ${totalBefore} ` +
            `to ${gap} in ${total}`,
        );
      }
    } finally {
      const failures = await scenario.stop();
      assertEquals(failures, [], failures.join("\n"));
    }
  },
);
