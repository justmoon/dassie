import { test } from "node:test";
import { assertEquals, assertThrows } from "@std/assert";
import { DEV_RATES } from "../rates.ts";
import {
  findScenario,
  invalid,
  ledgerNetwork,
  live,
  micros,
  minted,
  peripheryOf,
  poolDrops,
  resumes,
  type ScenarioDef,
  SCENARIOS,
  TOKEN_SCHEMES,
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
    "needs --xrpl fake: only the fake ledger's issuer mints DSUSD on demand",
  );
  assertEquals(
    unavailable({ ...def, mint: undefined }, false)?.includes("pool"),
    true,
  );
  assertEquals(unavailable(def, true), undefined);
});

test("the market maker's scenario with two targets: both nodes pinned to both", () => {
  const def = findScenario("market-maker-two-targets")!;
  assertEquals(invalid(def), undefined);
  assertEquals(peripheryOf(def, 0), undefined);
  assertEquals(peripheryOf(def, 1), undefined);
  assertEquals(peripheryOf(def, 2)?.uplinks, [0, 1]);
  assertEquals(peripheryOf(def, 3)?.uplinks, [0, 1]);
  assertEquals(def.schemes.map((schemes) => minted(def, schemes)), [
    true,
    true,
    false,
    true,
  ]);
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

// spec/decisions/mainnet-schemes.md: a node runs only its realm's schemes,
// so a live scenario's nodes run only mainnet's, and a test scenario's none
// of them; its token is its network's dollar. The live realm runs only
// against the fake ledger's mainnet, and a live scenario's nodes start from
// scratch each run, since mainnet never resets.
test("a live scenario runs mainnet's schemes on the fake ledger's mainnet only", () => {
  const mainnet = ["xrpl", "xrpl-rlusd"];
  const ids = SCENARIOS.filter(live).map((def) => def.id);
  assertEquals(ids, ["xrpl-live-settlement", "rlusd-live-settlement"]);
  for (const def of SCENARIOS) {
    for (const schemes of [...def.schemes, def.addedSchemes]) {
      for (const scheme of schemes) {
        if (scheme === "stub") continue;
        assertEquals(
          mainnet.includes(scheme),
          live(def),
          `${def.id} ${scheme}`,
        );
      }
    }
    for (const token of [def.mint?.token, def.pool?.token]) {
      if (token === undefined) continue;
      assertEquals(token === "RLUSD", live(def), def.id);
    }
    assertEquals(ledgerNetwork(def), live(def) ? "mainnet" : "testnet");
    assertEquals(resumes(def), !live(def));
  }
  const rlusd = findScenario("rlusd-live-settlement")!;
  assertEquals(TOKEN_SCHEMES.RLUSD, "xrpl-rlusd");
  assertEquals(rlusd.schemes.map((schemes) => minted(rlusd, schemes)), [
    true,
    true,
  ]);
  const xrp = findScenario("xrpl-live-settlement")!;
  assertEquals(
    unavailable(xrp, false),
    "needs --xrpl fake: the live realm runs only against the fake ledger's mainnet, never with real money",
  );
  assertEquals(unavailable(xrp, true), undefined);
  assertEquals(unavailable(rlusd, true), undefined);
});

// spec/areas/xrpl.md#the-peg-guard: a dollar token's guard fails closed
// without a price, which dev timers make seconds, so every scenario whose
// nodes settle in a token makes that token's pool, priced at the dev rates.
test("every scenario that settles in a dollar token has its pool", () => {
  for (const def of SCENARIOS) {
    const schemes = [...def.schemes, def.addedSchemes].flat();
    for (const [token, scheme] of Object.entries(TOKEN_SCHEMES)) {
      if (!schemes.includes(scheme)) continue;
      assertEquals(def.pool?.token, token, `${def.id} has no ${token} pool`);
    }
  }
});
