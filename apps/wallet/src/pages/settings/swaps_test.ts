import { test } from "node:test";
import { assertEquals } from "@std/assert";
import { assetLedgers, parseTarget, targetsFrom, targetText } from "./swaps.ts";

test("a hub's ledger is no asset of its own", () => {
  assertEquals(
    assetLedgers([
      "stub+usd",
      "hub-xrpl-testnet-xrp+xrp",
      "xrpl-testnet-xrp+xrp",
    ]),
    ["stub+usd", "xrpl-testnet-xrp+xrp"],
  );
});

test("a target reads as a percentage, more than none and at most all", () => {
  assertEquals(parseTarget(""), null);
  assertEquals(parseTarget(" 40 % "), 400_000);
  assertEquals(parseTarget("0.0001"), 1);
  assertEquals(parseTarget("100"), 1_000_000);
  assertEquals(parseTarget("0"), undefined);
  assertEquals(parseTarget("100.0001"), undefined);
  assertEquals(parseTarget("1.23456"), undefined);
  assertEquals(parseTarget("-5"), undefined);
  assertEquals(targetText(400_000), "40");
  assertEquals(targetText(123_450), "12.345");
});

test("the targets typed, or the ledger whose target is refused", () => {
  assertEquals(
    targetsFrom([["stub+usd", "40"], ["xrpl-testnet-xrp+xrp", ""]]),
    { targets: [{ ledger: "stub+usd", targetPpm: 400_000 }] },
  );
  assertEquals(
    targetsFrom([["stub+usd", "40"], ["xrpl-testnet-xrp+xrp", "x"]]),
    { invalid: "xrpl-testnet-xrp+xrp" },
  );
});
