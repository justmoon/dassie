import { test } from "node:test";
import { assertEquals } from "@std/assert";
import {
  ageText,
  chargeText,
  parseCharge,
  rateText,
  sourceName,
} from "./rates.ts";

test("a rate reads as what a dollar buys, and its age", () => {
  assertEquals(rateText("XRP", "1.8656"), "1 USD = 1.8656 XRP");
  assertEquals(ageText(1_000n, 21_000, false), "just now");
  assertEquals(ageText(1_000n, 61_000, false), "1 minute ago");
  assertEquals(ageText(1_000n, 1_000 + 30 * 60_000, false), "30 minutes ago");
  assertEquals(
    ageText(0n, 0, true),
    "stale: payments that need it are refused",
  );
});

test("a source is named by its URL, or its kind", () => {
  assertEquals(sourceName("kraken", ""), "Kraken");
  assertEquals(
    sourceName("coinbase", "https://localhost:10100/rates.json"),
    "https://localhost:10100/rates.json",
  );
  assertEquals(sourceName("other", ""), "other");
});

test("fees and spreads are percentages up to 10", () => {
  assertEquals(parseCharge(""), 0);
  assertEquals(parseCharge("0"), 0);
  assertEquals(parseCharge("0.1"), 1_000);
  assertEquals(parseCharge("0.25 %"), 2_500);
  assertEquals(parseCharge("10"), 100_000);
  assertEquals(parseCharge("10.0001"), undefined);
  assertEquals(parseCharge("-1"), undefined);
  assertEquals(parseCharge("0.00001"), undefined);
  assertEquals(chargeText(2_500), "0.25");
  assertEquals(chargeText(0), "0");
  assertEquals(chargeText(100_000), "10");
});
