/**
 * Each peer's grace limit (D18) as the owner reads it: what a peer cost,
 * why a node was banned, and the share the owner sets.
 */

import { currencyOf, formatAmount } from "@dassie/ui/currency";

/** What a peer cost, as the node reports it. */
export interface Cost {
  assetCode: string;
  assetScale: number;
  refused: bigint;
  overdue: bigint;
  grace: bigint;
}

/** A ban, as the node reports it. */
export interface BanInfo {
  reason: string;
  bannedAtUnixMs: bigint;
  cost?: Cost;
}

/** How recent an automatic ban the dashboard tells the owner about. */
export const RECENT_BAN_MS = 7 * 24 * 60 * 60 * 1000;

const money = (value: bigint, cost: Cost) =>
  formatAmount(value, currencyOf(cost.assetCode, cost.assetScale)).text;

/** Everything a peer cost, against its limit: `$ 0.12 of $ 1.10`. */
export function costText(cost: Cost): string {
  return `${money(cost.refused + cost.overdue, cost)} of ${
    money(cost.grace, cost)
  }`;
}

/** What it is made of, when anything: refusals, and what is overdue. */
export function costParts(cost: Cost): string {
  const parts = [];
  if (cost.refused > 0n) parts.push(`${money(cost.refused, cost)} refused`);
  if (cost.overdue > 0n) parts.push(`${money(cost.overdue, cost)} overdue`);
  return parts.join(", ");
}

/** Why a node is banned, for people. */
export function banText(ban: BanInfo): string {
  switch (ban.reason) {
    case "owner":
      return "Banned by you.";
    case "grace-limit": {
      if (!ban.cost) return "Banned automatically, for what it cost.";
      const cost = ban.cost;
      return `Banned automatically: it cost ${
        money(cost.refused + cost.overdue, cost)
      } in 30 days, over its grace limit of ${money(cost.grace, cost)}.`;
    }
    default:
      return "Banned.";
  }
}

/** The nodes banned automatically lately, newest first. */
export function recentAutomaticBans<T extends { id: string; ban?: BanInfo }>(
  nodes: readonly T[],
  nowMs: number,
): T[] {
  return nodes
    .filter((node) =>
      node.ban?.reason === "grace-limit" &&
      Number(node.ban.bannedAtUnixMs) > nowMs - RECENT_BAN_MS
    )
    .sort((a, b) =>
      Number((b.ban?.bannedAtUnixMs ?? 0n) - (a.ban?.bannedAtUnixMs ?? 0n))
    );
}

/** The share of the credit, in parts per million, as a percentage. */
export function shareText(ppm: number): string {
  const percent = ppm / 10_000;
  return `${Number.isInteger(percent) ? percent : percent.toFixed(2)}%`;
}

/** A percentage the owner typed, in parts per million; `undefined` if it
 * is not one the node takes (more than 0, at most 1000%). */
export function parseShare(text: string): number | undefined {
  const trimmed = text.trim().replace(/%$/, "").trim();
  if (!/^\d+(\.\d{1,4})?$/.test(trimmed)) return undefined;
  const [whole, fraction = ""] = trimmed.split(".");
  const ppm = Number(whole) * 10_000 + Number(fraction.padEnd(4, "0"));
  return ppm > 0 && ppm <= 10_000_000 ? ppm : undefined;
}
