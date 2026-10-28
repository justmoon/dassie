/**
 * The targets a periphery node's own swaps aim for
 * (spec/decisions/peer-swaps.md), apart from the page: which ledgers name an
 * asset, and a target as the owner types it.
 */

/**
 * The ledgers among `ids` that name an asset: every one but a settlement
 * hub's, which counts with the owner book that funds the hub.
 */
export function assetLedgers(ids: readonly string[]): string[] {
  return ids.filter((id) => !id.startsWith("hub-"));
}

/** A target in parts per million, as a percentage. */
export function targetText(ppm: number): string {
  return `${(ppm / 10_000).toFixed(4).replace(/\.?0+$/, "")}`;
}

/**
 * A target the owner typed as a percentage, in parts per million: `null`
 * for none, `undefined` if it is not one the node takes (more than 0, at
 * most 100%, to four decimals).
 */
export function parseTarget(text: string): number | null | undefined {
  const trimmed = text.trim().replace(/%$/, "").trim();
  if (trimmed === "") return null;
  if (!/^\d+(\.\d{1,4})?$/.test(trimmed)) return undefined;
  const [whole, fraction = ""] = trimmed.split(".");
  const ppm = Number(whole) * 10_000 + Number(fraction.padEnd(4, "0"));
  return ppm > 0 && ppm <= 1_000_000 ? ppm : undefined;
}

/** A target as the contract carries it. */
export interface Target {
  ledger: string;
  targetPpm: number;
}

/**
 * The targets the owner typed, per ledger, or the first ledger whose target
 * the node would not take.
 */
export function targetsFrom(
  typed: readonly (readonly [string, string])[],
): { targets: Target[] } | { invalid: string } {
  const targets: Target[] = [];
  for (const [ledger, text] of typed) {
    const ppm = parseTarget(text);
    if (ppm === undefined) return { invalid: ledger };
    if (ppm !== null) targets.push({ ledger, targetPpm: ppm });
  }
  return { targets };
}
