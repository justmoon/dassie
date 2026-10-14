import { assertEquals } from "@std/assert";
import {
  amountText,
  currencyOf,
  formatAmount,
  parseAmount,
} from "./currency.ts";

const usd = currencyOf("USD", 9);

Deno.test("amounts show their currency, cut and not rounded", () => {
  const amount = formatAmount(1_234_567_899_999n, usd);
  assertEquals(amount.text, "$ 1,234.56");
  assertEquals([amount.fraction, amount.extra], ["56", "78"]);
  assertEquals(formatAmount(-5n, usd).text, "-$ 0.00");
  assertEquals(formatAmount(0n, currencyOf("XRP", 6)).text, "XRP 0.00");
  assertEquals(formatAmount(1234n, currencyOf("ABC", 0)).text, "ABC 1,234");
});

Deno.test("typed amounts", () => {
  assertEquals(parseAmount("5", 9), { ok: true, value: 5_000_000_000n });
  // A comma is a decimal point: 1,50 is one and a half, never 150.
  assertEquals(parseAmount("1,50", 2), { ok: true, value: 150n });
  assertEquals(parseAmount("1,500", 3), { ok: true, value: 1_500n });
  assertEquals(parseAmount("1,000.25", 2), { ok: false, reason: "invalid" });
  assertEquals(parseAmount("1,000,000", 2), { ok: false, reason: "invalid" });
  assertEquals(parseAmount("9223372036.854775808", 9), {
    ok: false,
    reason: "too-large",
  });
  assertEquals(parseAmount(".5", 2), { ok: true, value: 50n });
  assertEquals(parseAmount("", 2), { ok: false, reason: "empty" });
  assertEquals(parseAmount("-1", 2), { ok: false, reason: "negative" });
  assertEquals(parseAmount("1.234", 2), { ok: false, reason: "too-precise" });
  assertEquals(parseAmount("1e3", 2), { ok: false, reason: "invalid" });
  assertEquals(parseAmount(".", 2), { ok: false, reason: "invalid" });
});

Deno.test("amounts as typed again", () => {
  assertEquals(amountText(1_500_000_000n, 9), "1.5");
  assertEquals(amountText(10_000_000_000n, 9), "10");
  assertEquals(amountText(5n, 2), "0.05");
  assertEquals(parseAmount(amountText(123_456_789n, 9), 9), {
    ok: true,
    value: 123_456_789n,
  });
});
