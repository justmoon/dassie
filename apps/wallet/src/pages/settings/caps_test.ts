import { assertEquals } from "@std/assert";
import { currencyOf } from "../../shared/amount/currency.ts";
import { formatCap, usedShare, windowLabel } from "./caps.ts";

const usd = currencyOf("USD", 9);

Deno.test("caps in words", () => {
  assertEquals(formatCap(undefined, usd), "No limit");
  assertEquals(
    formatCap({ amount: 10_000_000_000n, windowMs: 86_400_000n }, usd),
    "$ 10.00 per day",
  );
  assertEquals(windowLabel(7_200_000n), "per 2 h");
  assertEquals(windowLabel(90_000n), "per 90 s");
});

Deno.test("the share of a cap used", () => {
  assertEquals(usedShare({ amount: 1000n }, 1000n), 0);
  assertEquals(usedShare({ amount: 1000n }, 250n), 0.75);
  assertEquals(usedShare({ amount: 1000n }, 0n), 1);
});
