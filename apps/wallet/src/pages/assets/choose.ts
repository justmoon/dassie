/**
 * Choosing assets (spec/areas/wallet.md#the-gate): what the checklist
 * offers, what starts ticked, and what an asset that needs another ticks
 * and locks (spec/decisions/owner-chooses-assets.md). The node says what
 * each asset needs; the wallet holds no list of its own.
 */

import { type AssetInfo, Realm } from "@dassie/rpc/proto";

/** The assets to choose from: those the node offers and does not hold. */
export function offered(assets: readonly AssetInfo[]): AssetInfo[] {
  return assets.filter((asset) => !asset.active);
}

/** Whether the wallet lets the owner choose an asset: only the testnet's
 * for now, whose money is not real. */
export function choosable(asset: AssetInfo): boolean {
  return asset.realm === Realm.TEST;
}

/** What starts ticked: XRP, where it is offered and may be chosen. */
export function initialChoice(rows: readonly AssetInfo[]): string[] {
  return rows.some((asset) => asset.name === "xrp" && choosable(asset))
    ? ["xrp"]
    : [];
}

/** The checklist as it stands: each row ticked, and, for a row ticked
 * because a ticked asset needs it, the assets that need it. */
export interface Choice {
  /** The names to add, in the rows' order. */
  ticked: string[];
  /** A locked row's name, and the names of the ticked assets that need
   * it. */
  neededBy: Record<string, string[]>;
}

/** The checklist for what the owner ticked, `chosen`: each asset a ticked
 * one needs, among the rows, is ticked too, and locked. A need the node
 * already holds is no row. */
export function choice(
  rows: readonly AssetInfo[],
  chosen: readonly string[],
): Choice {
  const names = new Set(rows.map((asset) => asset.name));
  const neededBy: Record<string, string[]> = {};
  // Needs of needs, until nothing more is ticked; bounded by the rows.
  let ticked = new Set(chosen.filter((name) => names.has(name)));
  for (let round = 0; round < rows.length; round++) {
    const next = new Set(ticked);
    for (const asset of rows) {
      if (!ticked.has(asset.name)) continue;
      for (const need of asset.needs) {
        if (!names.has(need) || need === asset.name) continue;
        next.add(need);
        const by = (neededBy[need] ??= []);
        if (!by.includes(asset.name)) by.push(asset.name);
      }
    }
    if (next.size === ticked.size) break;
    ticked = next;
  }
  return {
    ticked: rows.map((asset) => asset.name).filter((name) => ticked.has(name)),
    neededBy,
  };
}

/** `chosen` with `name` ticked or unticked by the owner. */
export function toggle(chosen: readonly string[], name: string): string[] {
  return chosen.includes(name)
    ? chosen.filter((entry) => entry !== name)
    : [...chosen, name];
}
