import { test } from "node:test";
import { assert, assertEquals, assertThrows } from "@std/assert";
import {
  dsusdAt,
  dsusdBeside,
  FEE_UNITS_MAX,
  FEE_UNITS_PER_ONE,
  gap,
  midPrice,
  type Pool,
  swapOut,
  withdrawCap,
} from "./amm.ts";
import type { Decimal } from "./units.ts";

const XRP = 1_000_000n;
const DSUSD = 1_000_000n;
const whole = (n: bigint): Decimal => ({ mantissa: n, exponent: 0 });

/** 1,000 XRP and 2,500 DSUSD: $2.50 an XRP, all the keeper's. */
const POOL: Pool = {
  drops: 1_000n * XRP,
  micros: 2_500n * DSUSD,
  lpTotal: whole(1_581_138n),
  lpKeeper: whole(1_581_138n),
  feeUnits: 300,
};

/** A small deterministic generator (xorshift64*), for the property tests:
 * fixed seeds only (spec/engineering.md#no-fuzzing-or-random-seeds-in-ci). */
function rng(seed: bigint): (below: bigint) => bigint {
  let state = seed;
  const mask = (1n << 64n) - 1n;
  return (below) => {
    state ^= state >> 12n;
    state ^= (state << 25n) & mask;
    state ^= state >> 27n;
    return ((state * 0x2545f4914f6cdd1dn) & mask) % below;
  };
}

test("the pool's DSUSD at a price, and its mid price", () => {
  assertEquals(dsusdAt(1_000n * XRP, 2_500_000n), 2_500n * DSUSD);
  assertEquals(dsusdAt(3n, 2_500_000n), 7n);
  assertEquals(midPrice(POOL), 2_500_000n);
});

test("gap: within the band nothing moves; outside it, exactly the difference", () => {
  assertEquals(gap(POOL, 2_500_000n, 2_000n), { side: "in-band" });
  // 0.2% off is still in the band; just past it is not.
  assertEquals(gap(POOL, 2_505_000n, 2_000n), { side: "in-band" });
  assertEquals(gap(POOL, 2_506_000n, 2_000n), {
    side: "low",
    micros: 6n * DSUSD,
  });
  assertEquals(gap(POOL, 2_400_000n, 2_000n), {
    side: "high",
    micros: 100n * DSUSD,
  });
  // With no band, any difference moves it.
  assertEquals(gap(POOL, 2_500_001n, 0n), { side: "low", micros: 1_000n });
});

test("gap: moving the DSUSD side by the gap lands on the target", () => {
  const next = rng(7n);
  for (let i = 0; i < 1_000; i += 1) {
    const pool = {
      ...POOL,
      drops: 1n * XRP + next(1_000_000n * XRP),
      micros: 1n * DSUSD + next(1_000_000n * DSUSD),
    };
    const price = 10_000n + next(1_000_000_000n);
    const to = gap(pool, price, 0n);
    const micros = to.side === "low"
      ? pool.micros + to.micros
      : to.side === "high"
      ? pool.micros - to.micros
      : pool.micros;
    assertEquals(micros, dsusdAt(pool.drops, price));
  }
});

test("swapOut: the constant product holds, and the fee keeps some back", () => {
  const out = swapOut(10n * XRP, POOL.drops, POOL.micros, 0);
  // Without a fee, the product never falls.
  assert(
    (POOL.drops + 10n * XRP) * (POOL.micros - out) >= POOL.drops * POOL.micros,
  );
  assert(
    (POOL.drops + 10n * XRP) * (POOL.micros - out - 1n) <
      POOL.drops * POOL.micros,
  );
  const withFee = swapOut(10n * XRP, POOL.drops, POOL.micros, 300);
  assert(withFee < out);
  assertEquals(swapOut(0n, POOL.drops, POOL.micros, 300), 0n);
  assertThrows(() => swapOut(1n, POOL.drops, POOL.micros, FEE_UNITS_MAX + 1));
  const next = rng(11n);
  for (let i = 0; i < 1_000; i += 1) {
    const inBalance = 1n + next(10n ** 15n);
    const outBalance = 1n + next(10n ** 15n);
    const amount = next(10n ** 15n);
    const fee = Number(next(BigInt(FEE_UNITS_MAX) + 1n));
    const paid = swapOut(amount, inBalance, outBalance, fee);
    assert(paid < outBalance);
    assert(paid <= swapOut(amount, inBalance, outBalance, 0));
  }
});

test("withdrawCap: the keeper's share of the DSUSD, and nothing without one", () => {
  assertEquals(withdrawCap(POOL), POOL.micros);
  assertEquals(
    withdrawCap({ ...POOL, lpKeeper: whole(1_581_138n / 4n) }),
    (POOL.micros * (1_581_138n / 4n)) / 1_581_138n,
  );
  assertEquals(withdrawCap({ ...POOL, lpKeeper: whole(0n) }), 0n);
  assertEquals(
    withdrawCap({
      ...POOL,
      lpTotal: { mantissa: 25n, exponent: 5 },
      lpKeeper: whole(625_000n),
    }),
    625n * DSUSD,
  );
});

/** `floor(sqrt(n))`, by Newton's method. */
function isqrt(n: bigint): bigint {
  if (n < 2n) return n;
  let x = n;
  let y = (x + 1n) / 2n;
  while (y < x) {
    x = y;
    y = (x + n / x) / 2n;
  }
  return x;
}

test("withdrawCap: the LP tokens a withdrawal within the share burns are within the share", () => {
  // The ledger's single-asset withdrawal burns a share x of the LP tokens,
  // the smaller root of x² − c·x + r = 0, c = r·f + 2 − f; amm.ts argues
  // that x ≤ r. Checked here at a scale of 10^18, rounding x up.
  const S = 10n ** 18n;
  const next = rng(13n);
  for (let i = 0; i < 2_000; i += 1) {
    const r = 1n + next(S);
    const f = (next(BigInt(FEE_UNITS_MAX) + 1n) * S) / FEE_UNITS_PER_ONE;
    const c = (r * f) / S + 2n * S - f;
    const root = isqrt(c * c - 4n * r * S);
    const x = (c - root + 1n) / 2n;
    assert(x <= r + 1n, `r ${r} f ${f}: x ${x}`);
  }
});

test("dsusdBeside: the DSUSD for a two-asset deposit, rounded up", () => {
  assertEquals(dsusdBeside(100n * XRP, POOL), 250n * DSUSD);
  assertEquals(dsusdBeside(1n, POOL), 3n);
});
