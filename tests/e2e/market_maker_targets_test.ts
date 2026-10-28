/**
 * End to end (spec/areas/testing.md#end-to-end): `dassie market-maker`
 * with several targets, on the `market-maker-two-targets` scenario: its
 * four validators found an XRP hub and a DSUSD hub on the fake ledger,
 * beside its XRP/DSUSD pool at the dev rate; two core nodes short of DSUSD
 * join both, and so does the market maker's periphery node, pinned to
 * both. Its node holds both core nodes' snapshots, and the market maker,
 * started from its config file, makes a trade through one of them.
 *
 * That it trades through each and moves each toward balance is checked in
 * Rust beside daemons on paused time (`crates/dassie-cli/tests/market_maker.rs`,
 * spec/areas/testing.md#the-market-maker-and-peer-swaps-on-paused-time).
 */

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Scenario } from "@dassie/dev/scenario";

import {
  DASSIE,
  FAKE_LEDGER,
  missingScenarioTools,
  PORT_OFFSETS,
} from "./harness.ts";
import {
  marketMakerNodes,
  quoted,
  tradesOnce,
  writeConfig,
} from "./market_maker.ts";
import { joinedHubs } from "./settling.ts";

/** Clear of every other file's ports (`PORT_OFFSETS`, which ports_test.ts checks). */
const PORT_OFFSET = PORT_OFFSETS["market_maker_targets_test.ts"];

const missing = await missingScenarioTools();

test(
  "dassie market-maker pinned to two core nodes trades through one",
  { skip: missing },
  async () => {
    const scenario = await Scenario.temporary("market-maker-two-targets", {
      dassie: DASSIE,
      fakeLedger: FAKE_LEDGER,
      portOffset: PORT_OFFSET,
    });
    const dir = await mkdtemp(join(tmpdir(), "dassie-market-maker-targets-"));
    try {
      const { cores, makerNode } = marketMakerNodes(scenario);
      assert(cores.length === 2, "two core nodes");
      await joinedHubs(scenario);
      await quoted(scenario, cores, makerNode);
      const file = await writeConfig(scenario, dir, makerNode);
      await tradesOnce(scenario, file, makerNode);
    } finally {
      await rm(dir, { recursive: true, force: true });
      const failures = await scenario.stop();
      assertEquals(failures, [], failures.join("\n"));
    }
  },
);
