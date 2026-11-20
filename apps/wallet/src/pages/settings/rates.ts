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
  ["norges-bank", "Norges Bank"],
  ["bank-of-canada", "Bank of Canada"],
  ["cnb", "Czech National Bank"],
  ["xrpl-pool", "XRP Ledger: mainnet's XRP/RLUSD pool"],
] as const;

/** The kinds of rate source that take another server, with what the field
 * says: Coinbase's format, served by others too, and the XRP/RLUSD pool,
 * followed on another XRPL server. */
export const URL_KINDS: Record<string, { hint: string; placeholder: string }> =
  {
    coinbase: {
      hint: "Empty for Coinbase's own; else an HTTPS server of its format.",
      placeholder: "https://api.coinbase.com/v2/exchange-rates?currency=USD",
    },
    "xrpl-pool": {
      hint:
        "Empty for Dassie's proof server; else an XRPL mainnet server that serves proofs.",
      placeholder: "wss://mainnet.xrpl.dassieland.net",
    },
  };

/** A rate source's name for people. */
export function sourceName(kind: string, url: string): string {
  if (url !== "") return url;
  return SOURCE_KINDS.find(([known]) => known === kind)?.[1] ?? kind;
}

/** Each kind's own server, as a sentence names it. */
const IN_A_SENTENCE: Record<string, string> = {
  ecb: "the European Central Bank",
  "bank-of-canada": "the Bank of Canada",
  cnb: "the Czech National Bank",
  "xrpl-pool": "mainnet's XRP/RLUSD pool on the XRP Ledger",
};

/** The line that names where the rates come from, as the central banks'
 * terms ask wherever a rate or a total is shown
 * (spec/decisions/open-rate-sources.md): "Rates from …, … and …."; empty
 * without a source. */
export function sourcesText(
  sources: readonly { kind: string; url: string }[],
): string {
  const names = sources.map((source) =>
    source.url === ""
      ? (IN_A_SENTENCE[source.kind] ?? sourceName(source.kind, ""))
      : source.url
  );
  if (names.length === 0) return "";
  const listed = names.length === 1
    ? names[0]
    : `${names.slice(0, -1).join(", ")} and ${names.at(-1)}`;
  return `Rates from ${listed}.`;
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
