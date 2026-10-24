import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { create } from "@bufbuild/protobuf";
import { LedgerPegSchema, PegPause } from "@dassie/rpc/proto";
import { lastPriceText, pegNotice, priceText } from "./peg.ts";

test("a price is cut to four decimals, never rounded toward a dollar", () => {
  assertEquals(priceText(1_000_000n), "$1.0000");
  assertEquals(priceText(950_000n), "$0.9500");
  assertEquals(priceText(999_999n), "$0.9999");
  assertEquals(priceText(1_030_099n), "$1.0300");
  assertEquals(priceText(0n), "$0.0000");
  assertEquals(priceText(12_345_678_901n), "$12345.6789");
});

test("a running guard is not told; a paused one says why", () => {
  const running = create(LedgerPegSchema, {
    pricePpm: 1_000_000n,
    ledgerIndex: 42,
  });
  assertEquals(pegNotice(running, "RLUSD", "XRP/RLUSD"), undefined);
  assertEquals(lastPriceText(running), "$1.0000, as of ledger 42");

  const off = create(LedgerPegSchema, {
    paused: PegPause.OFF_PEG,
    pricePpm: 950_000n,
    ledgerIndex: 43,
  });
  const notice = pegNotice(off, "RLUSD", "XRP/RLUSD");
  assertEquals(notice?.title, "Paused: RLUSD is off its dollar peg");
  assert(notice?.text.includes("XRP/RLUSD AMM pool prices it at $0.9500"));
  assert(notice?.text.includes("still booked"));

  const stale = create(LedgerPegSchema, { paused: PegPause.STALE });
  assertEquals(
    pegNotice(stale, "DSUSD", "XRP/DSUSD")?.title,
    "Paused: the price of DSUSD cannot be read",
  );
  assertEquals(lastPriceText(stale), undefined);
});
