import { assertEquals } from "@std/assert";
import { currencyOf, formatAmount, parseAmount } from "./currency.ts";

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
  assertEquals(parseAmount("1,000.25", 2), { ok: true, value: 100_025n });
  assertEquals(parseAmount(".5", 2), { ok: true, value: 50n });
  assertEquals(parseAmount("", 2), { ok: false, reason: "empty" });
  assertEquals(parseAmount("-1", 2), { ok: false, reason: "negative" });
  assertEquals(parseAmount("1.234", 2), { ok: false, reason: "too-precise" });
  assertEquals(parseAmount("1e3", 2), { ok: false, reason: "invalid" });
  assertEquals(parseAmount(".", 2), { ok: false, reason: "invalid" });
});
