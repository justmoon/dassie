import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { targetPrice, TICKER_AGE_MS_MAX } from "./price.ts";

const NOW_MS = 1_800_000_000_000;

/** A ticker answer as Bitstamp gives it, at `NOW_MS`. */
function ticker(fields: Record<string, unknown> = {}): unknown {
  return {
    timestamp: String(NOW_MS / 1000),
    open: "2.41000",
    high: "2.52000",
    low: "2.39000",
    last: "2.50110",
    volume: "1234567.89",
    vwap: "2.46",
    bid: "2.50100",
    ask: "2.50120",
    side: "0",
    open_24: "2.40",
    percent_change_24: "4.21",
    ...fields,
  };
}

test("the target is the mid between bid and ask, in millionths", () => {
  const target = targetPrice(ticker(), NOW_MS);
  assert(target.ok);
  assertEquals(target.value, {
    usdMicrosPerXrp: 2_501_100n,
    bidMicros: 2_501_000n,
    askMicros: 2_501_200n,
  });
});

test("a mid between two millionths rounds down", () => {
  const target = targetPrice(
    ticker({ bid: "2.500001", ask: "2.500002" }),
    NOW_MS,
  );
  assert(target.ok);
  assertEquals(target.value.usdMicrosPerXrp, 2_500_001n);
});

test("a stale ticker, or one from the future, is refused", () => {
  const old = String((NOW_MS - TICKER_AGE_MS_MAX - 1000) / 1000);
  assertEquals(targetPrice(ticker({ timestamp: old }), NOW_MS).ok, false);
  const ahead = String((NOW_MS + 120_000) / 1000);
  assertEquals(targetPrice(ticker({ timestamp: ahead }), NOW_MS).ok, false);
  assertEquals(
    targetPrice(ticker({ timestamp: NOW_MS / 1000 }), NOW_MS).ok,
    false,
  );
});

test("a broken answer is refused, never priced", () => {
  const broken: unknown[] = [
    null,
    "2.5",
    [],
    ticker({ bid: undefined }),
    ticker({ ask: 2.5 }),
    ticker({ bid: "abc" }),
    ticker({ bid: "0", ask: "0" }),
    ticker({ bid: "-1", ask: "2.5" }),
    // Crossed.
    ticker({ bid: "2.6", ask: "2.5" }),
    // Spread wider than allowed.
    ticker({ bid: "2.0", ask: "2.5" }),
    // Out of the believable range.
    ticker({ bid: "0.001", ask: "0.001" }),
    ticker({ bid: "5000", ask: "5000" }),
    ticker({ bid: "1e30", ask: "1e30" }),
  ];
  for (const answer of broken) {
    assertEquals(targetPrice(answer, NOW_MS).ok, false, JSON.stringify(answer));
  }
});
