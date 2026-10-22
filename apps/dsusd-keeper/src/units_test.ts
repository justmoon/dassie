import { test } from "node:test";
import { assertEquals, assertThrows } from "@std/assert";
import {
  ceilDiv,
  currencyHex,
  DSUSD_CURRENCY,
  DSUSD_MICROS_MAX,
  dsusdValue,
  floorDiv,
  floorUnits,
  formatUnits,
  mulRatioFloor,
  parseDecimal,
} from "./units.ts";

const decimal = (text: string) => {
  const parsed = parseDecimal(text);
  if (!parsed.ok) throw new Error(parsed.error);
  return parsed.value;
};

test("DSUSD's code is its letters in ASCII, padded to 160 bits", () => {
  assertEquals(DSUSD_CURRENCY, "4453555344000000000000000000000000000000");
  assertEquals(DSUSD_CURRENCY.length, 40);
  assertThrows(() => currencyHex("USD"));
  assertThrows(() => currencyHex("dsusd"));
});

test("parseDecimal reads the ledger's and the exchange's forms exactly", () => {
  assertEquals(decimal("12.5"), { mantissa: 125n, exponent: -1 });
  assertEquals(decimal("-3"), { mantissa: -3n, exponent: 0 });
  assertEquals(decimal("9999999999999999e80"), {
    mantissa: 9999999999999999n,
    exponent: 80,
  });
  assertEquals(decimal("1e-6"), { mantissa: 1n, exponent: -6 });
  assertEquals(decimal("1.25E+2"), { mantissa: 125n, exponent: 0 });
  for (
    const bad of [
      "",
      "1.",
      ".5",
      "1e",
      "0x10",
      "1,5",
      "NaN",
      "1e999",
      "1".repeat(70),
    ]
  ) {
    assertEquals(parseDecimal(bad).ok, false, bad);
  }
});

test("floorUnits rounds toward minus infinity", () => {
  assertEquals(floorUnits(decimal("1.2345678"), 6), 1_234_567n);
  assertEquals(floorUnits(decimal("-1.2345678"), 6), -1_234_568n);
  assertEquals(floorUnits(decimal("2.5e3"), 6), 2_500_000_000n);
  assertEquals(floorUnits(decimal("-0.0000001"), 6), -1n);
  assertEquals(floorUnits(decimal("0.0000001"), 6), 0n);
});

test("floorDiv and ceilDiv round as named", () => {
  assertEquals(floorDiv(7n, 2n), 3n);
  assertEquals(floorDiv(-7n, 2n), -4n);
  assertEquals(floorDiv(-6n, 2n), -3n);
  assertEquals(ceilDiv(7n, 2n), 4n);
  assertEquals(ceilDiv(6n, 2n), 3n);
  assertEquals(ceilDiv(0n, 2n), 0n);
});

test("formatUnits and parseDecimal round-trip", () => {
  for (const units of [0n, 1n, 10n, 1_000_000n, 1_234_567n, DSUSD_MICROS_MAX]) {
    const text = formatUnits(units, 6);
    assertEquals(floorUnits(decimal(text), 6), units, text);
  }
  assertEquals(formatUnits(1_500_000n, 6), "1.5");
  assertEquals(formatUnits(42n, 6), "0.000042");
});

test("dsusdValue refuses what the ledger could not hold exactly", () => {
  assertEquals(dsusdValue(DSUSD_MICROS_MAX), "9999999999.999999");
  assertThrows(() => dsusdValue(DSUSD_MICROS_MAX + 1n));
  assertThrows(() => dsusdValue(0n));
});

test("mulRatioFloor multiplies by a ratio of decimals exactly", () => {
  assertEquals(mulRatioFloor(1_000n, decimal("1"), decimal("3")), 333n);
  assertEquals(mulRatioFloor(1_000n, decimal("2e3"), decimal("4e3")), 500n);
  assertEquals(mulRatioFloor(1_000n, decimal("0.5"), decimal("5e2")), 1n);
  assertEquals(mulRatioFloor(1_000n, decimal("0"), decimal("5")), 0n);
});
