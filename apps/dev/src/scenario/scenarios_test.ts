import { test } from "node:test";
import { assertEquals, assertThrows } from "@std/assert";
import { DEV_RATES } from "../rates.ts";
import {
  findScenario,
  invalid,
  micros,
  minted,
  peripheryOf,
  poolDrops,
  type ScenarioDef,
  SCENARIOS,
  unavailable,
} from "./scenarios.ts";

test("every scenario's periphery nodes are pinned to its core nodes", () => {
  for (const def of SCENARIOS) {
    assertEquals(invalid(def), undefined, def.id);
  }
  const base: ScenarioDef = {
    id: "t",
    name: "t",
    description: "t",
    schemes: [["stub"], ["stub"], ["stub"]],
    addedSchemes: ["stub"],
    bootstrap: [0],
  };
  const pinned = (periphery: ScenarioDef["periphery"]) =>
    invalid({ ...base, periphery });
  assertEquals(pinned([{ node: 1, uplinks: [0] }]), undefined);
  assertEquals(pinned([{ node: 3, uplinks: [0] }]), "no node 3");
  assertEquals(
    pinned([{ node: 0, uplinks: [1] }]),
    "periphery node 0 is a bootstrap node",
  );
  assertEquals(
    pinned([{ node: 1, uplinks: [] }]),
    "periphery node 1 has no uplink",
  );
  assertEquals(
    pinned([{ node: 1, uplinks: [0, 0] }]),
    "periphery node 1 names an uplink twice",
  );
  assertEquals(
    pinned([{ node: 1, uplinks: [2] }, { node: 2, uplinks: [0] }]),
    "periphery node 1 is pinned to 2, no core node",
  );
  assertEquals(
    pinned([{ node: 1, uplinks: [0] }, { node: 1, uplinks: [0] }]),
    "a periphery node twice",
  );
});

test("the market maker's scenario: a core node and two pinned to it", () => {
  const def = findScenario("market-maker")!;
  assertEquals(peripheryOf(def, 0), undefined);
  assertEquals(peripheryOf(def, 1)?.uplinks, [0]);
  assertEquals(peripheryOf(def, 2)?.uplinks, [0]);
  // Only the nodes that run DSUSD's scheme are minted it.
  assertEquals(def.schemes.map((schemes) => minted(def, schemes)), [
    true,
    false,
    true,
  ]);
  assertEquals(minted(def, def.addedSchemes), true);
  assertEquals(
    unavailable(def, false),
    "needs --xrpl fake: only the fake ledger's issuer mints test DSUSD on demand",
  );
  assertEquals(
    unavailable({ ...def, mint: undefined }, false)?.includes("pool"),
    true,
  );
  assertEquals(unavailable(def, true), undefined);
});

test("a pool is priced at the dev rates, exactly", () => {
  assertEquals(micros("0.92"), 920_000n);
  assertEquals(micros("50000"), 50_000_000_000n);
  assertEquals(micros("1.0000001"), undefined);
  assertEquals(micros("-1"), undefined);
  assertEquals(micros("1e3"), undefined);
  // A dollar buys two XRP: 50000 dollars are 100000 XRP.
  assertEquals(poolDrops("50000", DEV_RATES.XRP), 100_000_000_000n);
  assertEquals(poolDrops("0.5", "0.92"), 460_000n);
  // Below a drop rounds down.
  assertEquals(poolDrops("0.000001", "0.5"), 0n);
  assertThrows(() => poolDrops("lots", "2"), Error, "no pool");
});
