/**
 * End to end (spec/areas/testing.md#end-to-end): a market maker trading
 * through several targets over settlement hubs
 * (spec/areas/rate-feed.md#market-maker). The
 * `market-maker-two-targets` scenario's four validators found an XRP hub
 * and a DSUSD hub on the fake ledger, beside its XRP/DSUSD pool at the dev
 * rate. Two core nodes, d1 and d2, join both, each with XRP and DSUSD,
 * most of its value in XRP; the market maker's two periphery nodes, each
 * pinned to both, join one each: one with XRP, one with DSUSD.
 *
 * Both core nodes lack DSUSD, so both feeds price it above the pool, alike,
 * at their curve's cap. `dassie market-maker`, from the e2e build, follows
 * both feeds, and pays DSUSD from its DSUSD node to its XRP node through
 * whichever pays best; targets that pay alike take turns. So both core
 * nodes take DSUSD in, through two nodes of the market maker's, each with
 * an uplink to both, and each holds a larger share of its value in DSUSD.
 */

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { ChildProcess } from "@dassie/dev/process";
import { Scenario } from "@dassie/dev/scenario";

import { DASSIE, FAKE_LEDGER, missingScenarioTools } from "./harness.ts";
import {
  DSUSD,
  feedPort,
  freshRates,
  held,
  marketMakerNodes,
  writeConfig,
  XRP,
} from "./market_maker.ts";
import { HUB_WITHIN_MS, joinedHubs, settledOverHubs } from "./settling.ts";

const SCENARIO = "market-maker-two-targets";
/**
 * How long the market maker may take to trade through both: it decides
 * every 10 s, a trade waits on settlement over both hubs, and once its XRP
 * node is above its band it swaps back through the pool before it trades
 * again.
 */
const RUN_MS = 600_000;
/**
 * Clear of every other file's ports, which run beside these, and below the
 * ports the system hands out (32768 up). The other scenario files take
 * `SCENARIO_PORT_OFFSET` up to 900 more, so their nodes' HTTP, HTTPS (and
 * QUIC) and feed ports fill 18000–20999 and their fake ledgers and rate
 * servers 24200–25210; dev_test.ts takes 16000, so 20001 up, 21001 up,
 * 22001 up and 26100–26210; Pebble takes 14000 and 15000. At 10000 this
 * file's nodes take 14001 up, 15001 up and 16001 up, and its fake ledger
 * and rate server 20200 and 20210 on TCP, where the others' feeds are UDP.
 */
const PORT_OFFSET = 10_000;

const missing = await missingScenarioTools();

test("a market maker moves each of two core nodes toward balance over hubs", {
  skip: missing,
}, async () => {
  const scenario = await Scenario.temporary(SCENARIO, {
    dassie: DASSIE,
    fakeLedger: FAKE_LEDGER,
    portOffset: PORT_OFFSET,
  });
  const dir = await mkdtemp(join(tmpdir(), "dassie-market-maker-targets-"));
  let maker: ChildProcess | undefined;
  try {
    const { cores, xrpNode, dsusdNode } = marketMakerNodes(scenario);
    assertEquals(cores.length, 2, "two core nodes");
    // Each member funds its accounts in its hubs from its owner books;
    // the core nodes peer with each other over both hubs, and each of the
    // market maker's nodes with both over the hub they share.
    await joinedHubs(scenario);
    for (const core of cores) {
      await scenario.until(
        `${core.name} has fresh rates`,
        () => freshRates(core),
      );
      await scenario.until(
        `${core.name} names its feed`,
        async () => (await feedPort(scenario, core)) === core.feedPort,
      );
    }

    const xrpBefore = await Promise.all(cores.map((node) => held(node, XRP)));
    const dsusdBefore = await Promise.all(
      cores.map((node) => held(node, DSUSD)),
    );
    const file = await writeConfig(
      scenario,
      dir,
      cores.map((core) => core.url),
      xrpNode,
      dsusdNode,
    );
    maker = new ChildProcess("market-maker", DASSIE, {
      args: ["market-maker", "--config", file],
      env: { DASSIE_LOG: "info" },
    });
    let exited = false;
    maker.exited.then(() => exited = true);
    try {
      // A trade's DSUSD reached each: only the market maker pays a core node
      // DSUSD, and a core node pays its own costs in it, such as the DSUSD
      // hub's fee on each claim it redeems, so what a trade brings shows as a
      // rise that may fall short of a whole trade. While one of a core node's
      // own moves between its hub and its book is on its way, the sum reads
      // short of what it holds, never above.
      await scenario.until(
        "the market maker trades through each core node",
        async () => {
          if (exited) throw new Error("the market maker exited");
          const dsusd = await Promise.all(
            cores.map((node) => held(node, DSUSD)),
          );
          return dsusd.every((held, i) => held > dsusdBefore[i]);
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
    await scenario.until(
      "settled",
      () => settledOverHubs(scenario),
      HUB_WITHIN_MS,
    );

    // Each core node holds a larger share of its value in DSUSD, the
    // asset it lacked, its hub and its owner book taken together: DSUSD
    // over XRP rose, which at one rate is its share.
    for (const [i, core] of cores.entries()) {
      const xrpAfter = await held(core, XRP);
      const dsusdAfter = await held(core, DSUSD);
      const went = `${core.name} went from ${dsusdBefore[i]} DSUSD and ` +
        `${xrpBefore[i]} XRP to ${dsusdAfter} DSUSD and ${xrpAfter} XRP`;
      assert(dsusdAfter * xrpBefore[i] > dsusdBefore[i] * xrpAfter, went);
    }
  } finally {
    await maker?.stop();
    await rm(dir, { recursive: true, force: true });
    const failures = await scenario.stop();
    assertEquals(failures, [], failures.join("\n"));
  }
});
