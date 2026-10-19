/**
 * BTP tokens' spending caps (spec/areas/connector.md#spending-caps) as the
 * owner sets and reads them.
 */

import { type Currency, formatAmount } from "@dassie/ui/currency";

/** The windows the owner picks from. */
export const WINDOWS = [
  { id: "hour", label: "per hour", ms: 3_600_000n },
  { id: "day", label: "per day", ms: 86_400_000n },
  { id: "week", label: "per week", ms: 604_800_000n },
] as const;

/** A window's words: one of ours, or the length. */
export function windowLabel(ms: bigint): string {
  const known = WINDOWS.find((window) => window.ms === ms);
  if (known) return known.label;
  const seconds = ms / 1000n;
  return seconds % 3600n === 0n
    ? `per ${seconds / 3600n} h`
    : `per ${seconds} s`;
}

/** A cap in words, e.g. `$ 10.00 per day`; "No limit" without one. */
export function formatCap(
  cap: { amount: bigint; windowMs: bigint } | undefined,
  currency: Currency,
): string {
  if (!cap) return "No limit";
  return `${formatAmount(cap.amount, currency).text} ${
    windowLabel(cap.windowMs)
  }`;
}

/** How much of a cap is used, as a share for a `<meter>`. */
export function usedShare(cap: { amount: bigint }, available: bigint): number {
  if (cap.amount === 0n) return 0;
  const used = cap.amount - (available > cap.amount ? cap.amount : available);
  return Number((used * 1000n) / cap.amount) / 1000;
}
