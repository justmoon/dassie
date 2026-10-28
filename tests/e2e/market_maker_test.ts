/**
 * End to end (spec/areas/testing.md#end-to-end): `dassie market-maker`, the
 * shipped binary, fits its node and the dev scenario. The `market-maker`
 * scenario's four validators found an XRP hub and a DSUSD hub on the fake
 * ledger, beside its XRP/DSUSD pool at the dev rate; a core node, d1, short
 * of DSUSD, joins both, and so does the market maker's periphery node,
 * pinned to it. The market maker starts from its config file and its node's
 * passphrase file, reads its node over the IPC socket, and makes a trade
 * through d1; SIGTERM stops it.
 *
 * What the market maker does to the core node and to its own value, with
 * one target and with two, is checked in Rust beside daemons on paused time
 * (`crates/dassie-cli/tests/market_maker.rs`,
 * spec/areas/testing.md#the-market-maker-and-peer-swaps-on-paused-time).
 */

import { test } from "node:test";
import { assertEquals } from "@std/assert";
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

const PORT_OFFSET = PORT_OFFSETS["market_maker_test.ts"];

const missing = await missingScenarioTools();

test(
  "dassie market-maker starts from its config, reads its node over IPC and trades through a core node",
  { skip: missing },
  async () => {
    const scenario = await Scenario.temporary("market-maker", {
      dassie: DASSIE,
      fakeLedger: FAKE_LEDGER,
      portOffset: PORT_OFFSET,
    });
    const dir = await mkdtemp(join(tmpdir(), "dassie-market-maker-"));
    try {
      const { cores, makerNode } = marketMakerNodes(scenario);
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
