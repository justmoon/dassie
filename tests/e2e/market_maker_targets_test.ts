/**
 * End to end (spec/areas/testing.md#end-to-end): a market maker trading
 * through several targets over settlement hubs
 * (spec/areas/rate-feed.md#market-maker). The
 * `market-maker-two-targets` scenario's four validators found an XRP hub
 * and a DSUSD hub on the fake ledger, beside its XRP/DSUSD pool at the dev
 * rate. Two core nodes, d1 and d2, join both, each with XRP and DSUSD,
 * most of its value in XRP; the market maker's periphery node, pinned to
 * both, joins both too.
 *
 * Both core nodes lack DSUSD, so both snapshots price it above the pool,
 * alike, at their curve's cap. `dassie market-maker`, from the e2e build,
 * reads both snapshots as its node holds them, and swaps DSUSD for XRP
 * through whichever pays best; targets that pay alike take turns. So both
 * core nodes take DSUSD in, through one node of the market maker's with an
 * uplink to both, and each holds a larger share of its value in DSUSD.
 */

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { ChildProcess } from "@dassie/dev/process";
import { Scenario } from "@dassie/dev/scenario";

import {
  DASSIE,
  FAKE_LEDGER,
  missingScenarioTools,
  PORT_OFFSETS,
} from "./harness.ts";
import {
  DSUSD,
  freshRates,
  held,
  marketMakerNodes,
  peerSnapshot,
  writeConfig,
  XRP,
} from "./market_maker.ts";
import { HUB_WITHIN_MS, joinedHubs, settledOverHubs } from "./settling.ts";

const SCENARIO = "market-maker-two-targets";
/**
 * How long the market maker may take to trade through both: it decides
 * every 10 s, a swap waits on settlement over both hubs, and once its XRP
 * is above its band it swaps back through the pool before it trades again.
 */
const RUN_MS = 600_000;
/** Clear of every other file's ports (`PORT_OFFSETS`, which ports_test.ts checks). */
const PORT_OFFSET = PORT_OFFSETS["market_maker_targets_test.ts"];

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
    const { cores, makerNode } = marketMakerNodes(scenario);
    assertEquals(cores.length, 2, "two core nodes");
    // Each member funds its accounts in its hubs from its owner books;
    // the core nodes peer with each other over both hubs, and the market
    // maker's node with both over both.
    await joinedHubs(scenario);
    for (const core of cores) {
      await scenario.until(
        `${core.name} has fresh rates`,
        () => freshRates(core),
      );
      await scenario.until(
        `the market maker's node holds ${core.name}'s snapshot`,
        async () => (await peerSnapshot(makerNode, core)) !== undefined,
      );
    }

    const xrpBefore = await Promise.all(cores.map((node) => held(node, XRP)));
    const dsusdBefore = await Promise.all(
      cores.map((node) => held(node, DSUSD)),
    );
    const file = await writeConfig(scenario, dir, makerNode);
    maker = new ChildProcess("market-maker", DASSIE, {
      args: ["market-maker", "--config", file],
      env: { DASSIE_LOG: "info" },
    });
    let exited = false;
    maker.exited.then(() => exited = true);
    try {
      // A swap's DSUSD reached each: only the market maker pays a core node
      // DSUSD, and a core node pays its own costs in it, such as the DSUSD
      // hub's fee on each claim it redeems, so what a swap brings shows as a
      // rise that may fall short of a whole one. While one of a core node's
      // own moves between its hub and its book is on its way, the sum reads
      // short of what it holds, never above.
      await scenario.until(
        "the market maker swaps through each core node",
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
