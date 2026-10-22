// The price the keeper holds the pool at: XRP/USD from one exchange's
// ticker (spec/decisions/dsusd.md), checked before it moves any money.

import {
  floorUnits,
  MICROS_PER_DSUSD,
  parseDecimal,
  type Result,
} from "./units.ts";

/** Bitstamp's XRP/USD ticker. */
export const TICKER_URL = "https://www.bitstamp.net/api/v2/ticker/xrpusd/";

/** The prices the keeper believes, in millionths of a dollar per XRP: an
 * answer outside them is a broken ticker, not a market, and moving the
 * pool there would mint or burn DSUSD for nothing. */
export const PRICE_MICROS_MIN = 10_000n; // $0.01
export const PRICE_MICROS_MAX = 1_000_000_000n; // $1,000
/** The widest spread between bid and ask the keeper takes, in millionths
 * of the mid: a wider one is a market too thin to price from. */
export const SPREAD_PPM_MAX = 20_000n;
/** How old a ticker may be, and how far ahead of our clock. */
export const TICKER_AGE_MS_MAX = 10 * 60_000;
export const TICKER_AHEAD_MS_MAX = 60_000;

/** What the keeper takes from the ticker, in millionths of a dollar per
 * XRP. The target is the mid between bid and ask. */
export type Target = {
  usdMicrosPerXrp: bigint;
  bidMicros: bigint;
  askMicros: bigint;
};

/** The target price from a ticker answer (parsed JSON), at `nowMs`. */
export function targetPrice(answer: unknown, nowMs: number): Result<Target> {
  if (typeof answer !== "object" || answer === null) {
    return { ok: false, error: "ticker: not an object" };
  }
  const fields = answer as Record<string, unknown>;
  const bid = priceField(fields, "bid");
  if (!bid.ok) return bid;
  const ask = priceField(fields, "ask");
  if (!ask.ok) return ask;
  const timestamp = fields["timestamp"];
  if (typeof timestamp !== "string" || !/^\d{1,12}$/.test(timestamp)) {
    return { ok: false, error: "ticker: no timestamp" };
  }
  const ageMs = nowMs - Number(timestamp) * 1000;
  if (ageMs > TICKER_AGE_MS_MAX || ageMs < -TICKER_AHEAD_MS_MAX) {
    return { ok: false, error: `ticker: ${ageMs} ms old` };
  }
  if (bid.value <= 0n || ask.value < bid.value) {
    return { ok: false, error: `ticker: bid ${bid.value} ask ${ask.value}` };
  }
  const mid = (bid.value + ask.value) / 2n;
  if (mid < PRICE_MICROS_MIN || mid > PRICE_MICROS_MAX) {
    return { ok: false, error: `ticker: ${mid} µUSD/XRP is out of bounds` };
  }
  if ((ask.value - bid.value) * 1_000_000n > SPREAD_PPM_MAX * mid) {
    return { ok: false, error: "ticker: spread too wide" };
  }
  return {
    ok: true,
    value: { usdMicrosPerXrp: mid, bidMicros: bid.value, askMicros: ask.value },
  };
}

/** A price field, a decimal string, in millionths of a dollar. */
function priceField(
  fields: Record<string, unknown>,
  name: string,
): Result<bigint> {
  const text = fields[name];
  if (typeof text !== "string") {
    return { ok: false, error: `ticker: no ${name}` };
  }
  const value = parseDecimal(text);
  if (!value.ok) return { ok: false, error: `ticker: ${name}: ${value.error}` };
  const micros = floorUnits(value.value, 6);
  // A price this far out is refused above; the bound keeps the arithmetic
  // small before that.
  if (micros > PRICE_MICROS_MAX * MICROS_PER_DSUSD) {
    return { ok: false, error: `ticker: ${name} is out of bounds` };
  }
  return { ok: true, value: micros };
}
