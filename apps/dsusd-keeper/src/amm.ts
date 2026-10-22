// The XRP/DSUSD pool's constant-product math (spec/areas/dsusd-keeper.md),
// exact: XRP in drops, DSUSD in millionths, the price in millionths of a
// dollar per XRP, and the trading fee in the ledger's units.

import {
  ceilDiv,
  type Decimal,
  DROPS_PER_XRP,
  mulRatioFloor,
} from "./units.ts";

/** The ledger's trading fee is counted in 1/100,000 of the amount in, and
 * is at most 1,000 (1%). */
export const FEE_UNITS_PER_ONE = 100_000n;
export const FEE_UNITS_MAX = 1_000;

/** The pool, as `amm_info` reports it, with the keeper's LP tokens. */
export type Pool = {
  drops: bigint;
  micros: bigint;
  /** All LP tokens outstanding, and those the keeper holds. */
  lpTotal: Decimal;
  lpKeeper: Decimal;
  feeUnits: number;
};

/** The DSUSD a pool of `drops` holds at `price`, rounded down: the pool's
 * DSUSD side at the target. */
export function dsusdAt(drops: bigint, usdMicrosPerXrp: bigint): bigint {
  return (drops * usdMicrosPerXrp) / DROPS_PER_XRP;
}

/** The pool's mid price, in millionths of a dollar per XRP, rounded
 * down. The trading fee lies around it either way. */
export function midPrice(pool: Pool): bigint {
  if (pool.drops <= 0n) throw new Error("midPrice: an empty pool");
  return (pool.micros * DROPS_PER_XRP) / pool.drops;
}

/** Where the pool's price is against the target. */
export type Gap =
  | { side: "in-band" }
  /** XRP is too cheap: the pool needs `micros` more DSUSD. */
  | { side: "low"; micros: bigint }
  /** XRP is too dear: the pool needs `micros` less DSUSD. */
  | { side: "high"; micros: bigint };

/** How far the pool's DSUSD is from the target's, at the pool's XRP. A
 * single-asset deposit or withdrawal of DSUSD moves the pool's DSUSD by
 * exactly its amount and leaves its XRP as it is (the fee is taken in the
 * LP tokens it issues or burns), so this gap is the amount to move. Within
 * `bandPpm` of the target, the pool is left alone. */
export function gap(pool: Pool, usdMicrosPerXrp: bigint, bandPpm: bigint): Gap {
  const want = dsusdAt(pool.drops, usdMicrosPerXrp);
  const diff = want - pool.micros;
  const size = diff < 0n ? -diff : diff;
  if (size * 1_000_000n <= bandPpm * want) return { side: "in-band" };
  return diff > 0n
    ? { side: "low", micros: diff }
    : { side: "high", micros: size };
}

/** The most DSUSD the keeper can withdraw on its own (single-asset) for
 * certain: its share of the pool's DSUSD, rounded down.
 *
 * Withdrawing a share r of one asset burns a share x of the LP tokens,
 * the smaller root of x² − c·x + r = 0 with c = r·f + 2 − f, at the
 * trading fee f. At x = r that polynomial is r·(r − 1)·(1 − f), which is
 * not positive for r ≤ 1 and f ≤ 1, so r lies between the roots: the
 * smaller one is at most r. A withdrawal within the keeper's share of the
 * DSUSD therefore never needs more LP tokens than the keeper holds, at
 * any fee and without the ledger's square root. */
export function withdrawCap(pool: Pool): bigint {
  if (pool.lpTotal.mantissa <= 0n || pool.lpKeeper.mantissa <= 0n) return 0n;
  const cap = mulRatioFloor(pool.micros, pool.lpKeeper, pool.lpTotal);
  return cap < pool.micros ? cap : pool.micros;
}

/** What a swap of `amountIn` into the pool pays out, with the trading
 * fee: the constant product, with the fee kept back from what goes in,
 * rounded down. */
export function swapOut(
  amountIn: bigint,
  balanceIn: bigint,
  balanceOut: bigint,
  feeUnits: number,
): bigint {
  if (amountIn < 0n || balanceIn <= 0n || balanceOut <= 0n) {
    throw new Error("swapOut: out of domain");
  }
  if (!Number.isInteger(feeUnits) || feeUnits < 0 || feeUnits > FEE_UNITS_MAX) {
    throw new Error(`swapOut: fee ${feeUnits} is out of range`);
  }
  const net = amountIn * (FEE_UNITS_PER_ONE - BigInt(feeUnits));
  return (balanceOut * net) / (balanceIn * FEE_UNITS_PER_ONE + net);
}

/** The DSUSD that goes with `drops` into a two-asset deposit at the pool's
 * ratio, rounded up: the ledger takes both at that ratio, so a little more
 * DSUSD than it needs makes the XRP the side that binds. */
export function dsusdBeside(drops: bigint, pool: Pool): bigint {
  if (pool.drops <= 0n) throw new Error("dsusdBeside: an empty pool");
  return ceilDiv(drops * pool.micros, pool.drops);
}
