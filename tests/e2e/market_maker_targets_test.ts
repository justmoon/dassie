/**
 * End to end (spec/areas/testing.md#end-to-end): a market maker trading
 * through several targets (spec/areas/rate-feed.md#market-maker). The
 * `market-maker-two-targets` scenario runs two core nodes, d1 and d2, each
 * with XRP and DSUSD, most of its value in XRP, and the market maker's two
 * periphery nodes each pinned to both: d3 with XRP, d4 with DSUSD. The
 * fake ledger holds an XRP/DSUSD pool at the dev rate.
 *
 * Both core nodes lack DSUSD, so both feeds price it above the pool, alike,
 * at their curve's cap. `dassie market-maker`, from the e2e build, follows
 * both feeds, and pays DSUSD from d4 to d3 through whichever pays best;
 * targets that pay alike take turns. So both core nodes take DSUSD in,
 * through two nodes of the market maker's, each with an uplink to both.
 */

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { LedgerConnection } from "@dassie/rpc/proto";
import { ChildProcess } from "@dassie/dev/process";
import {
  findScenario,
  Scenario,
  type ScenarioNode,
} from "@dassie/dev/scenario";

import {
  DASSIE,
  FAKE_LEDGER,
  missingScenarioTools,
  SCENARIO_PORT_OFFSET,
} from "./harness.ts";
import {
  DSUSD,
  feedPort,
  ledger,
  owner,
  peersOf,
  settled,
  TRADE,
  UNIT,
  writeConfig,
  XRP,
} from "./market_maker.ts";

const SCENARIO = "market-maker-two-targets";
/**
 * How long the market maker may take to trade through both: it decides
 * every 10 s, a trade waits on settlement both ways, and once d3 is above
 * its band it swaps back through the pool before it trades again.
 */
const RUN_MS = 600_000;
/**
 * Clear of every other file's ports, which run beside these: the other
 * files take offsets of `SCENARIO_PORT_OFFSET` up to 900 more, so their
 * nodes' HTTP, HTTPS (and QUIC) and feed ports fill 18000–20999 and their
 * fake ledgers and rate servers 24200–25210. A thousand more would put
 * this file's HTTP on another file's HTTPS, and its HTTPS and QUIC on
 * another's feed.
 */
const PORT_OFFSET = SCENARIO_PORT_OFFSET + 3_000;
/**
 * The peer accounts once settled, both sides of each: d3 and d4 with d1
 * and d2, at least; d1 and d2 peer with each other too.
 */
const PEER_ACCOUNTS = 8;

const missing = await missingScenarioTools();

/** Every node follows the ledger and is funded, and both core nodes have
 * fresh rates. */
async function funded(scenario: Scenario): Promise<boolean> {
  const [d1, d2, d3, d4] = scenario.nodes;
  const minted = BigInt(findScenario(SCENARIO)!.mint!.value) * UNIT;
  const views: [ScenarioNode, string, bigint][] = [
    [d1, XRP, 1n],
    [d1, DSUSD, minted],
    [d2, XRP, 1n],
    [d2, DSUSD, minted],
    [d3, XRP, 1n],
    [d4, DSUSD, minted],
  ];
  for (const [node, id, least] of views) {
    const view = await ledger(node, id);
    if (view?.connection !== LedgerConnection.SYNCED) return false;
    if (view.ownerBalance < least) return false;
  }
  for (const core of [d1, d2]) {
    const { rates } = await core.rpc.call("listLedgers", {});
    const fresh = ["XRP", "USD"].every((code) =>
      rates.some((rate) => rate.assetCode === code && !rate.stale)
    );
    if (!fresh) return false;
  }
  return true;
}

/** d3 and d4 are each peered with both core nodes, and only with them. */
async function peered(scenario: Scenario): Promise<boolean> {
  const [d1, d2, d3, d4] = scenario.nodes;
  const cores = [d1.id, d2.id].sort().join();
  for (const node of [d3, d4]) {
    if ((await peersOf(node)).join() !== cores) return false;
  }
  for (const core of [d1, d2]) {
    const peers = await peersOf(core);
    if (!peers.includes(d3.id) || !peers.includes(d4.id)) return false;
  }
  return true;
}

test("a market maker trades through each of two core nodes", {
  skip: missing !== undefined,
}, async () => {
  const scenario = await Scenario.temporary(SCENARIO, {
    dassie: DASSIE,
    fakeLedger: FAKE_LEDGER,
    portOffset: PORT_OFFSET,
  });
  const dir = await mkdtemp(join(tmpdir(), "dassie-market-maker-targets-"));
  let maker: ChildProcess | undefined;
  try {
    const [d1, d2, d3, d4] = scenario.nodes;
    await scenario.until("funded", () => funded(scenario), 120_000);
    await scenario.until("d3 and d4 peered with both", () => peered(scenario));
    for (const core of [d1, d2]) {
      await scenario.until(
        `${core.name} names its feed`,
        async () => (await feedPort(scenario, core)) === core.feedPort,
      );
    }
    // d4 pays both in advance, which extend it nothing until then
    // (spec/decisions/earned-trust.md).
    await scenario.until(
      "paid in advance",
      () => settled(scenario, PEER_ACCOUNTS),
      120_000,
    );

    const cores = [d1, d2];
    const xrpBefore = await Promise.all(cores.map((node) => owner(node, XRP)));
    const dsusdBefore = await Promise.all(
      cores.map((node) => owner(node, DSUSD)),
    );
    const file = await writeConfig(
      scenario,
      dir,
      { targets: [d1.url, d2.url] },
      d3,
      d4,
    );
    maker = new ChildProcess("market-maker", DASSIE, {
      args: ["market-maker", "--config", file],
      env: { DASSIE_LOG_LEVEL: "info" },
    });
    let exited = false;
    maker.exited.then(() => exited = true);
    try {
      // A trade's DSUSD reached each: only the market maker pays a core
      // node DSUSD.
      await scenario.until(
        "the market maker trades through d1 and through d2",
        async () => {
          if (exited) throw new Error("the market maker exited");
          const dsusd = await Promise.all(
            cores.map((node) => owner(node, DSUSD)),
          );
          return dsusd.every((held, i) => held >= dsusdBefore[i] + TRADE);
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
      () => settled(scenario, PEER_ACCOUNTS),
      180_000,
    );

    // Each core node holds a larger share of its value in DSUSD, the
    // asset it lacked: DSUSD over XRP rose, which at one rate is its share.
    for (const [i, core] of cores.entries()) {
      const xrpAfter = await owner(core, XRP);
      const dsusdAfter = await owner(core, DSUSD);
      assert(
        dsusdAfter * xrpBefore[i] > dsusdBefore[i] * xrpAfter,
        `${core.name} went from ${dsusdBefore[i]} DSUSD and ` +
          `${xrpBefore[i]} XRP to ${dsusdAfter} DSUSD and ${xrpAfter} XRP`,
      );
    }
  } finally {
    await maker?.stop();
    await rm(dir, { recursive: true, force: true });
    const failures = await scenario.stop();
    assertEquals(failures, [], failures.join("\n"));
  }
});
