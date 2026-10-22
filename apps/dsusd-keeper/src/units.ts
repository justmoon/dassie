// Exact amounts for the keeper (spec/areas/dsusd-keeper.md): XRP in drops,
// DSUSD in millionths, and the ledger's decimal values, never a float.

/** An expected failure, as a value. */
export type Result<T> = { ok: true; value: T } | { ok: false; error: string };

export const DROPS_PER_XRP = 1_000_000n;
/** DSUSD is counted in millionths of a dollar, as the node counts a
 * dollar token (spec/areas/xrpl.md#rlusd). */
export const DSUSD_DECIMALS = 6;
export const MICROS_PER_DSUSD = 1_000_000n;
/** The most DSUSD an amount the keeper sends may hold, in millionths: an
 * issued value has sixteen significant digits, so a larger one would not
 * be the amount the keeper computed. */
export const DSUSD_MICROS_MAX = 10n ** 16n - 1n;

/** The token's code. Codes of other than three letters go on the ledger as
 * 40 hex digits: the letters in ASCII, padded with zeros. */
export const DSUSD_CODE = "DSUSD";
export const DSUSD_CURRENCY = currencyHex(DSUSD_CODE);

/** `code` as the ledger's 160-bit currency code. A code of three letters
 * is the ledger's standard form and must not be written so. */
export function currencyHex(code: string): string {
  if (!/^[A-Z0-9]{4,20}$/.test(code)) {
    throw new Error(`currencyHex: ${code} is not 4 to 20 letters or digits`);
  }
  const hex = Buffer.from(code, "ascii").toString("hex").toUpperCase();
  return hex.padEnd(40, "0");
}

/** A decimal value, exactly: `mantissa × 10^exponent`. */
export type Decimal = { mantissa: bigint; exponent: number };

/** The largest exponent a parsed value may have, either way: the ledger's
 * values stay within ±96, and a bound keeps every power of ten small. */
export const EXPONENT_LIMIT = 200;
/** The longest decimal text read. */
const DECIMAL_TEXT_MAX = 64;

const DECIMAL = /^(-?)(\d{1,40})(?:\.(\d{1,40}))?(?:[eE]([+-]?\d{1,4}))?$/;

/** Parses a value as the ledger and the exchange write it ("12.5",
 * "-3", "9999999999999999e80", "1e-6"), exactly. */
export function parseDecimal(text: string): Result<Decimal> {
  if (text.length > DECIMAL_TEXT_MAX) {
    return { ok: false, error: `not a decimal: ${text.slice(0, 20)}…` };
  }
  const match = DECIMAL.exec(text);
  if (match === null) return { ok: false, error: `not a decimal: ${text}` };
  const [, sign = "", whole = "", fraction = "", power = "0"] = match;
  const digits = BigInt(whole + fraction);
  const exponent = Number(power) - fraction.length;
  if (Math.abs(exponent) > EXPONENT_LIMIT) {
    return { ok: false, error: `exponent out of range: ${text}` };
  }
  return {
    ok: true,
    value: { mantissa: sign === "-" ? -digits : digits, exponent },
  };
}

/** `value × 10^decimals`, rounded down (toward minus infinity). */
export function floorUnits(value: Decimal, decimals: number): bigint {
  const shift = value.exponent + decimals;
  if (shift >= 0) return value.mantissa * 10n ** BigInt(shift);
  return floorDiv(value.mantissa, 10n ** BigInt(-shift));
}

/** `a / b` rounded toward minus infinity, for `b > 0`. */
export function floorDiv(a: bigint, b: bigint): bigint {
  if (b <= 0n) throw new Error("floorDiv: divisor must be positive");
  const quotient = a / b;
  return a % b !== 0n && a < 0n ? quotient - 1n : quotient;
}

/** `a / b` rounded up, for `a ≥ 0` and `b > 0`. */
export function ceilDiv(a: bigint, b: bigint): bigint {
  if (a < 0n || b <= 0n) throw new Error("ceilDiv: out of domain");
  return (a + b - 1n) / b;
}

/** `units / 10^decimals` as plain decimal text, without trailing zeros. */
export function formatUnits(units: bigint, decimals: number): string {
  if (units < 0n) throw new Error("formatUnits: negative");
  const scale = 10n ** BigInt(decimals);
  const whole = units / scale;
  const fraction = (units % scale).toString().padStart(decimals, "0")
    .replace(/0+$/, "");
  return fraction === "" ? whole.toString() : `${whole}.${fraction}`;
}

/** A DSUSD amount the keeper sends, as the ledger's decimal text. */
export function dsusdValue(micros: bigint): string {
  if (micros <= 0n || micros > DSUSD_MICROS_MAX) {
    throw new Error(`dsusdValue: ${micros} millionths is out of range`);
  }
  return formatUnits(micros, DSUSD_DECIMALS);
}

/** `floor(a × num / den)`, for `a ≥ 0`, `num ≥ 0` and `den > 0`. */
export function mulRatioFloor(a: bigint, num: Decimal, den: Decimal): bigint {
  if (a < 0n || num.mantissa < 0n || den.mantissa <= 0n) {
    throw new Error("mulRatioFloor: out of domain");
  }
  const shift = num.exponent - den.exponent;
  if (shift >= 0) {
    return (a * num.mantissa * 10n ** BigInt(shift)) / den.mantissa;
  }
  return (a * num.mantissa) / (den.mantissa * 10n ** BigInt(-shift));
}
