/**
 * End to end (spec/areas/testing.md#end-to-end): passive rebalancing, the
 * whole loop (spec/decisions/passive-rebalancing.md), with a market maker
 * of one node that swaps through a core node (spec/decisions/peer-swaps.md),
 * over settlement hubs. The `market-maker` scenario's four validators
 * found an XRP hub and a DSUSD hub on the fake ledger, beside its
 * XRP/DSUSD pool at the dev rate. A core node, d1, joins both, with XRP and
 * DSUSD, most of its value in XRP; the market maker's periphery node,
 * pinned to it, joins both too. Each funds its accounts in its hubs from
 * its owner books, and the two peer over both hubs, a method each.
 *
 * d1 lacks DSUSD, so its curve takes DSUSD in at better than spot
 * (spec/areas/connector.md#inventory-pricing), and its snapshot says so
 * (spec/areas/rate-feed.md); the market maker's node holds it, as it reads
 * its peers' snapshots. First the test is a reader itself: it swaps one
 * packet of DSUSD through d1 at what d1's snapshot promises, and gets no
 * less. Then `dassie market-maker`, from the e2e build, swaps DSUSD for
 * XRP through d1, and once its node holds more XRP than its band, swaps it
 * back into DSUSD through the pool from its owner book
 * (spec/areas/rate-feed.md#market-maker). So d1's share of its value in
 * DSUSD rises, toward its target; the market maker's value, at the pool's
 * price, grows; and every member's books are as they were after both
 * restart.
 */

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { ChildProcess } from "@dassie/dev/process";
import { SwapEnd } from "@dassie/rpc/proto";

import {
  DASSIE,
  FAKE_LEDGER,
  missingScenarioTools,
  PORT_OFFSETS,
} from "./harness.ts";
import {
  dollars,
  DSUSD,
  DSUSD_METHOD,
  freshRates,
  held,
  holdings,
  LARGEST_PACKET,
  marketMakerNodes,
  peerSnapshot,
  pool,
  promised,
  SWAP_TIMEOUT_MS,
  UNIT,
  value,
  writeConfig,
  XRP,
  XRP_METHOD,
} from "./market_maker.ts";
import {
  booksOverHubs,
  HUB_WITHIN_MS,
  joinedHubs,
  settledOverHubs,
} from "./settling.ts";
import {
  DEV_RATES,
  findScenario,
  poolDrops,
  Scenario,
} from "@dassie/dev/scenario";

/** The market maker's XRP band around what its node holds at the start: 3
 * XRP, less than the XRP a trade of 2 dollars brings it. */
const XRP_BAND = 3n * UNIT;

/**
 * How long the market maker may take to trade and swap back: it decides
 * every 10 s, and a swap waits on settlement over both hubs with d1.
 */
const RUN_MS = 300_000;
const PORT_OFFSET = PORT_OFFSETS["market_maker_test.ts"];

/**
 * What the reader swaps: a quarter of the largest packet of DSUSD, whose
 * XRP back, at about two XRP a dollar, is within the largest packet too,
 * so that it goes as one packet, the one the snapshot's promise is
 * computed for here.
 */
const READ_SWAP = LARGEST_PACKET / 4n;
/** How often the reader tries again when a newer snapshot promises less
 * than the one it read, and its swap paid nothing. */
const READ_TRIES = 5;

const missing = await missingScenarioTools();

test(
  "a market maker of one node swaps through a skewed core node over hubs, and gains",
  { skip: missing },
  async () => {
    const scenario = await Scenario.temporary("market-maker", {
      dassie: DASSIE,
      fakeLedger: FAKE_LEDGER,
      portOffset: PORT_OFFSET,
    });
    const dir = await mkdtemp(join(tmpdir(), "dassie-market-maker-"));
    let maker: ChildProcess | undefined;
    try {
      const { cores: [d1], makerNode } = marketMakerNodes(scenario);
      assert(d1, "a core node");
      // Each member funds its accounts in its hubs from its owner books, and
      // the market maker's node peers with d1 over both hubs.
      await joinedHubs(scenario);
      await scenario.until("d1 has fresh rates", () => freshRates(d1));
      // The market maker's node reads its peers' snapshots: it is set to
      // swap, as a new node is, though it swaps nothing by itself.
      const snapshot = () => peerSnapshot(makerNode, d1);
      await scenario.until(
        "the market maker's node holds d1's snapshot",
        async () => (await snapshot()) !== undefined,
      );
      const swaps = await makerNode.rpc.call("getSwapStatus", {});
      assert(!swaps.byItself, "a periphery node without targets");

      const seeded = await pool(scenario);
      const def = findScenario("market-maker")!;
      assertEquals(
        seeded.xrp,
        poolDrops(def.pool!.value, DEV_RATES.XRP) * 1_000n,
        "the pool at the dev rate",
      );
      const d1Xrp = await held(d1, XRP);
      const d1Dsusd = await held(d1, DSUSD);
      const before = await holdings(makerNode);

      // A reader never gets less than a snapshot it acted on promised
      // (spec/decisions/peer-swaps.md): one packet of DSUSD to d1, back in
      // XRP, at what the snapshot read says it gets, computed here from
      // its quotes and ladders. A newer snapshot may promise less, and the
      // node then sends nothing: it reads again and tries again.
      let read: { promised: bigint; received: bigint } | undefined;
      const tries: string[] = [];
      for (let attempt = 0; attempt < READ_TRIES && !read; attempt++) {
        const now = await snapshot();
        assert(now, "d1's snapshot");
        const least = promised(now, DSUSD_METHOD, XRP_METHOD, READ_SWAP);
        assert(least !== undefined && least > 0n, "d1 prices DSUSD to XRP");
        const done = await makerNode.rpc.call("swapWithPeer", {
          node: d1.id,
          payOn: DSUSD_METHOD,
          backOn: XRP_METHOD,
          amount: READ_SWAP,
          rateNumerator: least,
          rateDenominator: READ_SWAP,
        }, { timeoutMs: SWAP_TIMEOUT_MS });
        // Nothing paid: a newer snapshot promised less than the rate, or
        // the swap came back short of what the one it was sent on promised,
        // and the node took none of it.
        tries.push(
          JSON.stringify(
            { least, done, now },
            (_, v) => typeof v === "bigint" ? `${v}` : v,
          ),
        );
        if (done.paid === 0n) continue;
        assertEquals(done.paid, READ_SWAP, "one packet, all of it");
        assertEquals(done.fulfilled, 1n);
        assertEquals(done.ended, SwapEnd.DONE);
        read = { promised: least, received: done.received };
      }
      assert(read, `no swap paid in ${READ_TRIES} tries: ${tries.join("\n")}`);
      assert(
        read.received >= read.promised,
        `${read.received} came back, ${read.promised} promised`,
      );

      // A band narrower than what one trade brings the node's XRP, so that
      // it swaps back after the first.
      const file = await writeConfig(scenario, dir, makerNode, XRP_BAND);
      maker = new ChildProcess("market-maker", DASSIE, {
        args: ["market-maker", "--config", file],
        env: { DASSIE_LOG: "info" },
      });
      let exited = false;
      maker.exited.then(() => exited = true);
      try {
        // A swap's DSUSD reached d1, and the node's XRP went back through
        // the pool: the only swaps on it are the market maker's. Only the
        // market maker pays d1 DSUSD, and d1 pays its own costs in it,
        // such as the DSUSD hub's fee on each claim it redeems, so what a
        // swap brings shows as a rise that may fall short of a whole one.
        // While one of d1's own moves between its hub and its book is on
        // its way, the sum reads short of what it holds, never above.
        await scenario.until(
          "the market maker swaps through d1 and back through the pool",
          async () => {
            if (exited) throw new Error("the market maker exited");
            const dsusd = await held(d1, DSUSD);
            const now = await pool(scenario);
            return now.xrp > seeded.xrp && dsusd > d1Dsusd + READ_SWAP;
          },
          RUN_MS,
        );
      } catch (error) {
        const { output } = await maker.stop();
        maker = undefined;
        throw new Error(`${(error as Error).message}\n${output}`);
      }
      const { code, output } = await maker.stop();
      maker = undefined;
      // SIGTERM: it stops at once, as asked.
      assertEquals(code, 0, output);
      // What it paid may still be on its way: the nodes finish it, and
      // move funds between their hubs and books as they need.
      await scenario.until(
        "settled",
        () => settledOverHubs(scenario),
        HUB_WITHIN_MS,
      );
      const after = await makerNode.rpc.call("getSwapStatus", {});
      assert(after.fulfilled > 1n, "the market maker swapped");

      const d1XrpAfter = await held(d1, XRP);
      const d1DsusdAfter = await held(d1, DSUSD);
      const went = `d1 went from ${d1Dsusd} DSUSD and ${d1Xrp} XRP ` +
        `to ${d1DsusdAfter} DSUSD and ${d1XrpAfter} XRP`;
      // d1 holds a larger share of its value in DSUSD, the asset it
      // lacked, its hub and its owner book taken together: DSUSD over XRP
      // rose, which at one rate is its share.
      assert(d1DsusdAfter * d1Xrp > d1Dsusd * d1XrpAfter, went);
      assert(d1DsusdAfter > d1Dsusd, "d1 took DSUSD in");
      assert(d1XrpAfter < d1Xrp, "d1 paid XRP out");

      // The market maker holds more than it did, at the pool's price now.
      const gained = await holdings(makerNode);
      const now = await pool(scenario);
      assert(now.xrp > seeded.xrp && now.dsusd < seeded.dsusd, "it swapped");
      const [was, is] = [value(before, now), value(gained, now)];
      assert(
        is > was,
        `the market maker's value went from ${dollars(was, now)} dollars ` +
          `to ${dollars(is, now)}`,
      );

      // Restarted, both catch up and book nothing twice.
      const members = [d1, makerNode];
      const books = () =>
        Promise.all(members.map((node) => booksOverHubs(scenario, node)));
      const kept = await books();
      for (const node of members) {
        const { code, output } = (await node.stop())!;
        assertEquals(code, 0, output);
      }
      for (const node of members) {
        await node.start();
        await node.waitForEngine();
      }
      await scenario.until(
        "caught up",
        () => settledOverHubs(scenario),
        HUB_WITHIN_MS,
      );
      assertEquals(await books(), kept, "the books after a restart");
    } finally {
      await maker?.stop();
      await rm(dir, { recursive: true, force: true });
      const failures = await scenario.stop();
      assertEquals(failures, [], failures.join("\n"));
    }
  },
);
