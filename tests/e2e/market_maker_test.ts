/**
 * End to end (spec/areas/testing.md#end-to-end): passive rebalancing, the
 * whole loop (spec/decisions/passive-rebalancing.md). The `market-maker`
 * scenario runs a core node, d1, with XRP and DSUSD, most of its value in
 * XRP, and a market maker's two periphery nodes pinned to it: d2 with XRP,
 * d3 with DSUSD. The fake ledger holds an XRP/DSUSD pool at the dev rate.
 *
 * d1 lacks DSUSD, so its curve takes DSUSD in at better than spot
 * (spec/areas/connector.md#inventory-pricing), and its feed says so
 * (spec/areas/rate-feed.md). `dassie market-maker`, from the e2e build,
 * reads the feed, pays DSUSD from d3 through d1 to d2, which d1 converts
 * to XRP, and once d2 holds more XRP than its band, swaps it back into
 * DSUSD through the pool (spec/areas/rate-feed.md#market-maker). So d1's
 * share of its value in DSUSD rises, toward its target; the market
 * maker's value, at the pool's price, grows; and every node's books are
 * as they were after all three restart.
 */

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { LedgerConnection } from "@dassie/rpc/proto";
import { ChildProcess } from "@dassie/dev/process";

import {
  DASSIE,
  FAKE_LEDGER,
  missingScenarioTools,
  SCENARIO_PORT_OFFSET,
} from "./harness.ts";
import {
  books,
  dollars,
  DSUSD,
  feedPort,
  holdings,
  ledger,
  owner,
  peerAccounts,
  peersOf,
  pool,
  quiet,
  settled,
  TRADE,
  UNIT,
  value,
  writeConfig,
  XRP,
} from "./market_maker.ts";

/** d2's band around what it holds at the start: 3 XRP, less than the XRP a
 * trade of 2 dollars brings it. */
const XRP_BAND = 3n * UNIT;
import {
  DEV_RATES,
  findScenario,
  poolDrops,
  Scenario,
  type ScenarioNode,
} from "@dassie/dev/scenario";

/**
 * How long the market maker may take to trade and swap back: it decides
 * every 10 s, and a trade waits on settlement both ways through d1.
 */
const RUN_MS = 300_000;
const PORT_OFFSET = SCENARIO_PORT_OFFSET + 100;
/** The peer accounts once settled: d2 and d3 with d1, both sides. */
const PEER_ACCOUNTS = 4;

const missing = await missingScenarioTools();

/** Every node follows the ledger and is funded, and d1 has fresh rates. */
async function funded(scenario: Scenario): Promise<boolean> {
  const [d1, d2, d3] = scenario.nodes;
  const minted = BigInt(findScenario("market-maker")!.mint!.value) * UNIT;
  const views: [ScenarioNode, string, bigint][] = [
    [d1, XRP, 1n],
    [d1, DSUSD, minted],
    [d2, XRP, 1n],
    [d3, DSUSD, minted],
  ];
  for (const [node, id, least] of views) {
    const view = await ledger(node, id);
    if (view?.connection !== LedgerConnection.SYNCED) return false;
    if (view.ownerBalance < least) return false;
  }
  const { rates } = await d1.rpc.call("listLedgers", {});
  return ["XRP", "USD"].every((code) =>
    rates.some((rate) => rate.assetCode === code && !rate.stale)
  );
}

/** d2 and d3 are peered with d1, their only peer. */
async function peered(scenario: Scenario): Promise<boolean> {
  const [d1, d2, d3] = scenario.nodes;
  const ofD1 = await peersOf(d1);
  return ofD1.length === 2 && ofD1.includes(d2.id) && ofD1.includes(d3.id) &&
    (await peersOf(d2)).join() === d1.id &&
    (await peersOf(d3)).join() === d1.id;
}

test("a market maker moves a skewed core node toward balance, and gains", {
  skip: missing !== undefined,
}, async () => {
  const scenario = await Scenario.temporary("market-maker", {
    dassie: DASSIE,
    fakeLedger: FAKE_LEDGER,
    portOffset: PORT_OFFSET,
  });
  const dir = await mkdtemp(join(tmpdir(), "dassie-market-maker-"));
  let maker: ChildProcess | undefined;
  try {
    const [d1, d2, d3] = scenario.nodes;
    await scenario.until("funded", () => funded(scenario), 120_000);
    await scenario.until("d2 and d3 peered with d1", () => peered(scenario));
    await scenario.until(
      "d1 names its feed",
      async () => (await feedPort(scenario, d1)) === d1.feedPort,
    );
    // d3 pays d1 in advance, which extends it nothing until then
    // (spec/decisions/earned-trust.md).
    await scenario.until(
      "paid in advance",
      () => settled(scenario, PEER_ACCOUNTS),
      120_000,
    );

    const seeded = await pool(scenario);
    const def = findScenario("market-maker")!;
    assertEquals(
      seeded.xrp,
      poolDrops(def.pool!.value, DEV_RATES.XRP) * 1_000n,
      "the pool at the dev rate",
    );
    const d1Xrp = await owner(d1, XRP);
    const d1Dsusd = await owner(d1, DSUSD);
    const before = await holdings(d2, d3);

    // A band narrower than what one trade brings d2, so that it swaps back
    // after the first.
    const file = await writeConfig(
      scenario,
      dir,
      [d1.url],
      d2,
      d3,
      XRP_BAND,
    );
    maker = new ChildProcess("market-maker", DASSIE, {
      args: ["market-maker", "--config", file],
      env: { DASSIE_LOG: "info" },
    });
    let exited = false;
    maker.exited.then(() => exited = true);
    try {
      // A trade's DSUSD reached d1, and d2's XRP went back through the
      // pool: the only swaps on it are the market maker's.
      await scenario.until(
        "the market maker trades through d1 and swaps back through the pool",
        async () => {
          if (exited) throw new Error("the market maker exited");
          const now = await pool(scenario);
          return now.xrp > seeded.xrp &&
            (await owner(d1, DSUSD)) >= d1Dsusd + TRADE;
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
    // What it paid is still on its way: the nodes finish it.
    await scenario.until(
      "settled",
      () => settled(scenario, PEER_ACCOUNTS),
      180_000,
    );

    // d1 holds a larger share of its value in DSUSD, the asset it lacked:
    // DSUSD over XRP rose, which at one rate is its share.
    const d1XrpAfter = await owner(d1, XRP);
    const d1DsusdAfter = await owner(d1, DSUSD);
    assert(
      d1DsusdAfter * d1Xrp > d1Dsusd * d1XrpAfter,
      `d1 went from ${d1Dsusd} DSUSD and ${d1Xrp} XRP ` +
        `to ${d1DsusdAfter} DSUSD and ${d1XrpAfter} XRP`,
    );
    assert(d1DsusdAfter > d1Dsusd, "d1 took DSUSD in");
    assert(d1XrpAfter < d1Xrp, "d1 paid XRP out");

    // The market maker holds more than it did, at the pool's price now.
    const after = await holdings(d2, d3);
    const now = await pool(scenario);
    assert(now.xrp > seeded.xrp && now.dsusd < seeded.dsusd, "it swapped");
    const [was, is] = [value(before, now), value(after, now)];
    assert(
      is > was,
      `the market maker's value went from ${dollars(was, now)} dollars ` +
        `to ${dollars(is, now)}`,
    );

    // Restarted, all three catch up and book nothing twice.
    const kept = await books(scenario);
    const accounts = await peerAccounts(scenario);
    for (const node of scenario.nodes) {
      const { code, output } = (await node.stop())!;
      assertEquals(code, 0, output);
    }
    for (const node of scenario.nodes) {
      await node.start();
      await node.waitForEngine();
    }
    await scenario.until(
      "caught up",
      () => quiet(scenario, PEER_ACCOUNTS),
      120_000,
    );
    assertEquals(await books(scenario), kept, "the books after a restart");
    assertEquals(await peerAccounts(scenario), accounts, "peer accounts");
  } finally {
    await maker?.stop();
    await rm(dir, { recursive: true, force: true });
    const failures = await scenario.stop();
    assertEquals(failures, [], failures.join("\n"));
  }
});
