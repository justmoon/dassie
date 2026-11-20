// The price the keeper holds the pool at: XRP's in dollars, from mainnet's
// XRP/RLUSD pool (spec/decisions/open-rate-sources.md), checked before it
// moves any money.

import {
  currencyHex,
  type Decimal,
  DROPS_PER_XRP,
  MICROS_PER_DSUSD,
  mulRatioFloor,
  parseDecimal,
  type Result,
} from "./units.ts";

/** RLUSD on mainnet, as Ripple publishes it, the same as the node's
 * (`crates/dassie-xrpl/src/rlusd.rs`). */
export const RLUSD_CURRENCY = currencyHex("RLUSD");
export const RLUSD_ISSUER = "rMxCKbEDwqr76QuheSUMdEGf4B9xJ8m5De";

/** The least XRP the pool must hold for its price to count, in drops: the
 * floor the node's XRP rate takes for the same pool (`MIN_POOL_DROPS` in
 * `crates/dassie-xrpl/src/pool_rate.rs`). It holds about two million; drained
 * to a twentieth of that, it has lost the depth that makes its price
 * costly to move, and an emptied pool is filled again at whatever ratio
 * its first depositor picks. */
export const POOL_DROPS_MIN = 100_000n * DROPS_PER_XRP;
/** The prices the keeper believes, in millionths of a dollar per XRP: a
 * pool outside them is broken, not a market, and moving the testnet's pool
 * there would mint or burn DSUSD for nothing. */
export const PRICE_MICROS_MIN = 10_000n; // $0.01
export const PRICE_MICROS_MAX = 1_000_000_000n; // $1,000
/** How long ago the ledger read may have closed: as long as the node
 * takes a validated ledger as fresh (`FRESH_FOR_MS` in
 * `crates/dassie-xrpl/src/unl.rs`). A server in sync has one that closed
 * seconds ago; an older one is a server that fell behind. */
export const LEDGER_AGE_MS_MAX = 180_000;
/** How far ahead of our clock its close time may be: a close time is
 * rounded to the ledger's resolution, at most two minutes, so it can be
 * up to one ahead, and our clock may drift a little. */
export const LEDGER_AHEAD_MS_MAX = 90_000;
/** The XRP Ledger's epoch, 2000-01-01, in milliseconds since 1970. */
const RIPPLE_EPOCH_MS = 946_684_800_000;

/** What the keeper takes from the pool: XRP's price in millionths of a
 * dollar, RLUSD at par, and what it was read from. */
export type Target = {
  usdMicrosPerXrp: bigint;
  ledgerIndex: number;
  drops: bigint;
  rlusd: Decimal;
};

type Fields = Record<string, unknown>;

/** The target price from mainnet's answers, `ledger`'s result for a
 * validated ledger and `amm_info`'s result for the XRP/RLUSD pool at that
 * ledger (src/ledger.ts), at `nowMs`. */
export function targetPrice(
  ledger: unknown,
  amm: unknown,
  nowMs: number,
): Result<Target> {
  const header = ledgerOf(ledger);
  if (!header.ok) return header;
  const ageMs = nowMs - header.value.closedMs;
  if (ageMs > LEDGER_AGE_MS_MAX || ageMs < -LEDGER_AHEAD_MS_MAX) {
    return { ok: false, error: `price: ledger closed ${ageMs} ms ago` };
  }
  const answer = object(amm, "amm_info");
  if (!answer.ok) return answer;
  if (
    answer.value["validated"] !== true ||
    answer.value["ledger_hash"] !== header.value.hash
  ) {
    return { ok: false, error: "price: pool not read at the ledger" };
  }
  const pool = object(answer.value["amm"], "amm");
  if (!pool.ok) return pool;
  if (
    pool.value["asset_frozen"] === true ||
    pool.value["asset2_frozen"] === true
  ) {
    return { ok: false, error: "price: the pool is frozen" };
  }
  let drops: bigint | null = null;
  let rlusd: Decimal | null = null;
  for (const key of ["amount", "amount2"]) {
    const amount = pool.value[key];
    if (typeof amount === "string") {
      if (!/^\d{1,20}$/.test(amount)) {
        return { ok: false, error: `price: ${key}: not drops` };
      }
      drops = BigInt(amount);
      continue;
    }
    const token = object(amount, key);
    if (!token.ok) return token;
    if (
      token.value["currency"] !== RLUSD_CURRENCY ||
      token.value["issuer"] !== RLUSD_ISSUER
    ) {
      return { ok: false, error: `price: ${key}: not RLUSD` };
    }
    const text = token.value["value"];
    const value = typeof text === "string"
      ? parseDecimal(text)
      : { ok: false as const, error: "not a string" };
    if (!value.ok) {
      return { ok: false, error: `price: ${key}: ${value.error}` };
    }
    rlusd = value.value;
  }
  if (drops === null || rlusd === null) {
    return { ok: false, error: "price: not an XRP/RLUSD pool" };
  }
  if (drops < POOL_DROPS_MIN) {
    return { ok: false, error: `price: the pool holds only ${drops} drops` };
  }
  if (rlusd.mantissa <= 0n) {
    return { ok: false, error: "price: the pool holds no RLUSD" };
  }
  // RLUSD per drop, in millionths of a dollar per XRP: rlusd × 10^6 ×
  // 10^6 / drops, rounded down.
  const price = mulRatioFloor(DROPS_PER_XRP * MICROS_PER_DSUSD, rlusd, {
    mantissa: drops,
    exponent: 0,
  });
  if (price < PRICE_MICROS_MIN || price > PRICE_MICROS_MAX) {
    return { ok: false, error: `price: ${price} µUSD/XRP is out of bounds` };
  }
  return {
    ok: true,
    value: {
      usdMicrosPerXrp: price,
      ledgerIndex: header.value.index,
      drops,
      rlusd,
    },
  };
}

/** The validated ledger's index, hash and close time from `ledger`'s
 * result. */
function ledgerOf(
  result: unknown,
): Result<{ index: number; hash: string; closedMs: number }> {
  const fields = object(result, "ledger");
  if (!fields.ok) return fields;
  if (fields.value["validated"] !== true) {
    return { ok: false, error: "price: ledger not validated" };
  }
  const header = object(fields.value["ledger"], "ledger header");
  if (!header.ok) return header;
  const index = fields.value["ledger_index"];
  const hash = fields.value["ledger_hash"];
  const closeTime = header.value["close_time"];
  if (
    typeof index !== "number" || !Number.isSafeInteger(index) || index < 0 ||
    typeof hash !== "string" || !/^[0-9A-F]{64}$/.test(hash) ||
    header.value["ledger_hash"] !== hash ||
    typeof closeTime !== "number" || !Number.isSafeInteger(closeTime) ||
    closeTime < 0
  ) {
    return { ok: false, error: "price: no validated ledger" };
  }
  return {
    ok: true,
    value: { index, hash, closedMs: RIPPLE_EPOCH_MS + closeTime * 1000 },
  };
}

function object(value: unknown, what: string): Result<Fields> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? { ok: true, value: value as Fields }
    : { ok: false, error: `price: ${what}: not an object` };
}
