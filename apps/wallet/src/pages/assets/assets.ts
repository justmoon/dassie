/**
 * Assets as the wallet shows them (spec/areas/wallet.md#pages): each
 * asset's balance by place, with what each part sets aside, as the node
 * reports it (spec/decisions/one-balance-per-asset.md), and what the
 * external assets it settles through hold as their ledger says, never as
 * the books add it up (spec/decisions/external-assets.md).
 */

import type { AssetBalance, ExternalAsset, Holding } from "@dassie/rpc/proto";
import { type Currency, currencyOf } from "@dassie/ui/currency";
import { assetText, tokenScheme } from "../schemes.ts";

/** Each kind of asset: its name for people, and its ledger's own asset,
 * which a faucet gives and fees are paid in. */
const KINDS: Record<string, { name: string; native: string }> = {
  "xrpl-testnet-account": { name: "XRP Ledger Testnet account", native: "XRP" },
  "xrpl-account": { name: "XRP Ledger account", native: "XRP" },
};

/** An asset's name for people, from its ID: its kind, and which of them
 * it is when it is not the first. */
export function assetName(id: string): string {
  const [kind, index] = id.split("/");
  const name = KINDS[kind]?.name ?? kind;
  const number = Number(index);
  return Number.isInteger(number) && number > 0
    ? `${name} ${number + 1}`
    : name;
}

/** The ledger's own asset of an asset's kind, such as "XRP", if the
 * wallet knows the kind. */
export function nativeAsset(id: string): string | undefined {
  return KINDS[id.split("/")[0]]?.native;
}

/** The currency a holding is written in. */
export function holdingCurrency(holding: {
  assetCode: string;
  assetScale: number;
}): Currency {
  return currencyOf(holding.assetCode, holding.assetScale);
}

/** What the issuer stops of a holding; nothing when it stops nothing. */
export function freezeText(holding: {
  frozen: boolean;
  deepFrozen: boolean;
  globalFreeze: boolean;
}): string | undefined {
  if (holding.deepFrozen) {
    return "Its issuer froze this line deep: it neither pays nor receives, but to and from the issuer. The node pays nothing out of it, a move into a hub included, until the issuer lifts the freeze.";
  }
  if (holding.globalFreeze) {
    return "Its issuer froze the token for every holder: it moves only to and from the issuer. The node pays nothing out of it, a move into a hub included, until the issuer lifts the freeze.";
  }
  if (holding.frozen) {
    return "Its issuer froze this line: it pays only the issuer, and still receives. The node pays nothing out of it, a move into a hub included, until the issuer lifts the freeze.";
  }
  return undefined;
}

/** The external asset the scheme `scheme` settles through, if any. */
export function assetOf(
  scheme: string,
  assets: readonly ExternalAsset[],
): ExternalAsset | undefined {
  return assets.find((asset) => asset.settlementSchemes.includes(scheme));
}

/** What `asset` holds of `code`: the ledger's own asset, or a token of any
 * issuer. */
export function holdingOf(
  asset: ExternalAsset | undefined,
  code: string,
): Holding | undefined {
  return asset?.holdings.find((holding) => holding.assetCode === code);
}

/** An asset's name for people: "Test XRP", or its code where the wallet
 * has no name for it. */
export function assetTitle(
  balance: Pick<AssetBalance, "name" | "realm">,
): string {
  return assetText(balance.name, balance.realm)?.label ??
    balance.name.toUpperCase();
}

/** The scheme of a ledger, from its ID, `<scheme>+<asset>`. */
export function ledgerScheme(id: string): string {
  return id.split("+")[0];
}

/** Whether a ledger is a settlement hub's: its scheme's ID starts with
 * `hub-` (spec/decisions/scheme-names.md). */
export function isHubLedger(id: string): boolean {
  return id.startsWith("hub-");
}

/** An asset's owner book, the ledger of the owner's own account, and its
 * settlement hub's ledger, by ID, each if it has one. */
export function assetLedgers(
  balance: Pick<AssetBalance, "ledgers">,
): { book?: string; hub?: string } {
  return {
    book: balance.ledgers.find((id) => !isHubLedger(id)),
    hub: balance.ledgers.find(isHubLedger),
  };
}

/** A part of a line for people: text, or an amount in the asset. */
export type Segment = string | { amount: bigint };

/** A part of an asset's balance, by place. */
export interface BreakdownRow {
  id: "account" | "hub" | "on-its-way";
  label: string;
  amount: bigint;
  /** What it sets aside, under it; empty for nothing to say. */
  sub: Segment[];
}

/** Amounts with what they are, the zero ones left out, as a list. */
function parts(entries: readonly [bigint, string][]): Segment[] {
  const shown = entries.filter(([amount]) => amount > 0n);
  return shown.flatMap(([amount, what], index) => [
    { amount },
    `${what}${index === shown.length - 1 ? "." : ", "}`,
  ]);
}

/**
 * An asset's balance by place: the owner's own account, with its reserve;
 * the settlement hub, with what is to spend, what is held for peers and
 * the hub's reserves; and a deposit on its way into the hub. A place the
 * asset has no ledger in is left out, so the stub's dollars, which have
 * neither, have no rows: their headline is all there is.
 */
export function breakdownRows(balance: AssetBalance): BreakdownRow[] {
  const { book, hub } = assetLedgers(balance);
  const rows: BreakdownRow[] = [];
  const bookScheme = book && ledgerScheme(book);
  if (bookScheme !== undefined && bookScheme !== "stub") {
    const token = tokenScheme(bookScheme);
    const ledger = bookScheme.startsWith("xrpl")
      ? "the XRP Ledger"
      : "the ledger";
    rows.push({
      id: "account",
      label: "In your account",
      amount: balance.account,
      sub: token
        ? [`Its reserve is in ${token.gas}, counted with your ${token.gas}.`]
        : balance.accountReserve > 0n
        ? [{ amount: balance.accountReserve }, ` of it is ${ledger}'s reserve.`]
        : [],
    });
  }
  if (hub !== undefined) {
    const aside = balance.heldForPeers + balance.hubReserve;
    const toSpend = balance.hub > aside ? balance.hub - aside : 0n;
    rows.push({
      id: "hub",
      label: "In the hub",
      amount: balance.hub,
      sub: parts([
        [toSpend, " to spend"],
        [balance.heldForPeers, " held for your peers"],
        [balance.hubReserve, " the hub's reserves"],
      ]),
    });
  }
  if (balance.onItsWay > 0n) {
    rows.push({
      id: "on-its-way",
      label: "On its way into the hub",
      amount: balance.onItsWay,
      sub: ["A deposit the hub has not credited yet."],
    });
  }
  return rows;
}

/** Whether a line under an asset's balance says what is spendable: where
 * a payment may spend less than the balance. */
export function showsSpendable(
  balance: Pick<AssetBalance, "ownerBalance" | "spendable">,
): boolean {
  return balance.ownerBalance > 0n && balance.spendable < balance.ownerBalance;
}
