/**
 * End to end (spec/areas/testing.md#end-to-end): passive rebalancing, the
 * whole loop (spec/decisions/passive-rebalancing.md), over settlement
 * hubs. The `market-maker` scenario's four validators found an XRP hub and
 * a DSUSD hub on the fake ledger, beside its XRP/DSUSD pool at the dev
 * rate. A core node, d1, joins both, with XRP and DSUSD, most of its value
 * in XRP; the market maker's two periphery nodes, pinned to it, join one
 * each: one with XRP, one with DSUSD. Each funds its accounts in its hubs
 * from its owner books, and peers with d1 over the hub they share.
 *
 * d1 lacks DSUSD, so its curve takes DSUSD in at better than spot
 * (spec/areas/connector.md#inventory-pricing), and its feed says so
 * (spec/areas/rate-feed.md). `dassie market-maker`, from the e2e build,
 * reads the feed, pays DSUSD over the DSUSD hub from its DSUSD node
 * through d1, which converts it, to its XRP node over the XRP hub, and once
 * that node holds more XRP than its band, swaps it back into DSUSD through
 * the pool from its owner book (spec/areas/rate-feed.md#market-maker). So
 * d1's share of its value in DSUSD rises, toward its target; the market
 * maker's value, at the pool's price, grows; and every member's books are
 * as they were after all three restart.
 */

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { ChildProcess } from "@dassie/dev/process";

import {
  DASSIE,
  FAKE_LEDGER,
  missingScenarioTools,
  SCENARIO_PORT_OFFSET,
} from "./harness.ts";
import {
  dollars,
  DSUSD,
  feedPort,
  freshRates,
  held,
  holdings,
  marketMakerNodes,
  pool,
  UNIT,
  value,
  writeConfig,
  XRP,
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

/** The market maker's XRP node's band around what it holds at the start: 3
 * XRP, less than the XRP a trade of 2 dollars brings it. */
const XRP_BAND = 3n * UNIT;

/**
 * How long the market maker may take to trade and swap back: it decides
 * every 10 s, and a trade waits on settlement over both hubs through d1.
 */
const RUN_MS = 300_000;
const PORT_OFFSET = SCENARIO_PORT_OFFSET + 100;

const missing = await missingScenarioTools();

test(
  "a market maker moves a skewed core node toward balance over hubs, and gains",
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
      const { cores: [d1], xrpNode, dsusdNode } = marketMakerNodes(scenario);
      // Each member funds its accounts in its hubs from its owner books, and
      // peers with d1 over the hub they share.
      await joinedHubs(scenario);
      await scenario.until("d1 has fresh rates", () => freshRates(d1));
      await scenario.until(
        "d1 names its feed",
        async () => (await feedPort(scenario, d1)) === d1.feedPort,
      );

      const seeded = await pool(scenario);
      const def = findScenario("market-maker")!;
      assertEquals(
        seeded.xrp,
        poolDrops(def.pool!.value, DEV_RATES.XRP) * 1_000n,
        "the pool at the dev rate",
      );
      const d1Xrp = await held(d1, XRP);
      const d1Dsusd = await held(d1, DSUSD);
      const before = await holdings(xrpNode, dsusdNode);

      // A band narrower than what one trade brings the XRP node, so that it
      // swaps back after the first.
      const file = await writeConfig(
        scenario,
        dir,
        [d1.url],
        xrpNode,
        dsusdNode,
        XRP_BAND,
      );
      maker = new ChildProcess("market-maker", DASSIE, {
        args: ["market-maker", "--config", file],
        env: { DASSIE_LOG: "info" },
      });
      let exited = false;
      maker.exited.then(() => exited = true);
      try {
        // A trade's DSUSD reached d1, and the XRP node's went back through the
        // pool: the only swaps on it are the market maker's. Only the market
        // maker pays d1 DSUSD, and d1 pays its own costs in it, such as the
        // DSUSD hub's fee on each claim it redeems, so what a trade brings
        // shows as a rise that may fall short of a whole trade. While one of
        // d1's own moves between its hub and its book is on its way, the sum
        // reads short of what it holds, never above.
        await scenario.until(
          "the market maker trades through d1 and swaps back through the pool",
          async () => {
            if (exited) throw new Error("the market maker exited");
            const dsusd = await held(d1, DSUSD);
            const now = await pool(scenario);
            return now.xrp > seeded.xrp && dsusd > d1Dsusd;
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
      // What it paid is still on its way: the nodes finish it, and move
      // funds between their hubs and books as they need.
      await scenario.until(
        "settled",
        () => settledOverHubs(scenario),
        HUB_WITHIN_MS,
      );

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
      const after = await holdings(xrpNode, dsusdNode);
      const now = await pool(scenario);
      assert(now.xrp > seeded.xrp && now.dsusd < seeded.dsusd, "it swapped");
      const [was, is] = [value(before, now), value(after, now)];
      assert(
        is > was,
        `the market maker's value went from ${dollars(was, now)} dollars ` +
          `to ${dollars(is, now)}`,
      );

      // Restarted, all three catch up and book nothing twice.
      const members = [d1, xrpNode, dsusdNode];
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
