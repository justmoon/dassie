/**
 * Exchange rates and charges (spec/decisions/exchange-rates.md), apart from
 * the pages: how rates read, how old they are, and the owner's fee and spread
 * as percentages.
 */

/** The currencies the node keeps rates for, and can show totals in. */
export const CURRENCIES = [
  "USD",
  "XRP",
  "EUR",
  "GBP",
  "JPY",
  "CHF",
  "CAD",
  "AUD",
] as const;

/** The kinds of rate source, with their names for people. */
export const SOURCE_KINDS = [
  ["coinbase", "Coinbase (or a server of its format)"],
  ["kraken", "Kraken"],
  ["bitstamp", "Bitstamp"],
  ["ecb", "European Central Bank"],
] as const;

/** A rate source's name for people. */
export function sourceName(kind: string, url: string): string {
  if (url !== "") return url;
  return SOURCE_KINDS.find(([known]) => known === kind)?.[1] ?? kind;
}

/** One dollar in a currency, as the node has it: "1 USD = 1.8656 XRP". */
export function rateText(code: string, perUsd: string): string {
  return `1 USD = ${perUsd} ${code}`;
}

/** How old a rate is, or that it is stale. */
export function ageText(asOfMs: bigint, nowMs: number, stale: boolean): string {
  if (stale) return "stale: payments that need it are refused";
  const seconds = Math.max(0, Math.round((nowMs - Number(asOfMs)) / 1000));
  if (seconds < 60) return "just now";
  const minutes = Math.round(seconds / 60);
  return `${minutes} minute${minutes === 1 ? "" : "s"} ago`;
}

/** A charge in parts per million, as a percentage. */
export function chargeText(ppm: number): string {
  return `${(ppm / 10_000).toFixed(4).replace(/\.?0+$/, "")}`;
}

/** A percentage the owner typed for a fee, a spread or the rebalancing, in
 * parts per million; `undefined` if it is not one the node takes (0 to
 * 10%). */
export function parseCharge(text: string): number | undefined {
  const trimmed = text.trim().replace(/%$/, "").trim();
  if (trimmed === "") return 0;
  if (!/^\d+(\.\d{1,4})?$/.test(trimmed)) return undefined;
  const [whole, fraction = ""] = trimmed.split(".");
  const ppm = Number(whole) * 10_000 + Number(fraction.padEnd(4, "0"));
  return ppm <= 100_000 ? ppm : undefined;
}
