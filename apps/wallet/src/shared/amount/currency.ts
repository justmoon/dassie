/**
 * Amounts in a ledger's own currency (UI-2): every amount the wallet shows
 * carries its asset, and is exact, never a float.
 *
 * As in the original, an amount shows `displayPrecision` digits after the
 * point, then two more muted ones, and is cut rather than rounded.
 */

/** How to show amounts of an asset. */
export interface Currency {
  code: string;
  symbol: string;
  /** The asset's scale: an amount of 1 is 10^-scale units. */
  scale: number;
  /** Digits after the point shown in full. */
  displayPrecision: number;
  /** Digits shown muted after those. */
  extraDigits: number;
}

const SYMBOLS: Record<string, string> = {
  USD: "$",
  EUR: "€",
  GBP: "£",
  XRP: "XRP",
};

/** The currency of an asset. */
export function currencyOf(code: string, scale: number): Currency {
  const displayPrecision = Math.min(2, scale);
  return {
    code,
    symbol: SYMBOLS[code] ?? code,
    scale,
    displayPrecision,
    extraDigits: Math.min(2, scale - displayPrecision),
  };
}

/** An amount, in parts. */
export interface FormattedAmount {
  negative: boolean;
  symbol: string;
  /** Grouped, as the locale groups. */
  integer: string;
  /** The digits shown in full after the point. */
  fraction: string;
  /** The muted digits after those. */
  extra: string;
  /** All of it as text, for people and tests. */
  text: string;
}

const pow10 = (n: number) => 10n ** BigInt(n);

/** Formats `value`, in the asset's smallest units. */
export function formatAmount(
  value: bigint,
  currency: Currency,
  locale = "en-US",
): FormattedAmount {
  const negative = value < 0n;
  const magnitude = negative ? -value : value;
  const unit = pow10(currency.scale);
  const integer = magnitude / unit;
  const digits = (magnitude % unit).toString().padStart(currency.scale, "0");
  const fraction = digits.slice(0, currency.displayPrecision);
  const extra = digits.slice(
    currency.displayPrecision,
    currency.displayPrecision + currency.extraDigits,
  );
  const grouped = new Intl.NumberFormat(locale, { useGrouping: true })
    .format(integer);
  const symbol = currency.symbol;
  const point = fraction.length > 0 ? `.${fraction}` : "";
  return {
    negative,
    symbol,
    integer: grouped,
    fraction,
    extra,
    text: `${negative ? "-" : ""}${symbol} ${grouped}${point}`,
  };
}

/** Why a typed amount is not one. */
export type ParseFailure = "empty" | "invalid" | "negative" | "too-precise";

/**
 * Parses an amount typed by a person, in units of the asset: digits, an
 * optional point and up to `scale` decimals, with optional group commas.
 */
export function parseAmount(
  text: string,
  scale: number,
): { ok: true; value: bigint } | { ok: false; reason: ParseFailure } {
  const trimmed = text.trim().replaceAll(",", "");
  if (trimmed === "") return { ok: false, reason: "empty" };
  if (trimmed.startsWith("-")) return { ok: false, reason: "negative" };
  const parts = /^(\d*)(?:\.(\d*))?$/.exec(trimmed);
  if (!parts || (parts[1] === "" && (parts[2] ?? "") === "")) {
    return { ok: false, reason: "invalid" };
  }
  const decimals = parts[2] ?? "";
  if (decimals.length > scale) return { ok: false, reason: "too-precise" };
  const whole = BigInt(parts[1] === "" ? "0" : parts[1]);
  const fraction = BigInt(decimals.padEnd(scale, "0") || "0");
  return { ok: true, value: whole * pow10(scale) + fraction };
}
