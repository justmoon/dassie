/**
 * External assets as the wallet shows them (spec/areas/wallet.md#pages):
 * what each holds as its ledger says, never as the books add it up
 * (spec/decisions/external-assets.md).
 */

import type { ExternalAsset, Holding } from "@dassie/rpc/proto";
import { type Currency, currencyOf } from "@dassie/ui/currency";

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
    return "Its issuer froze this line deep: it neither pays nor receives. Peers cannot pay you on it until the issuer lifts the freeze.";
  }
  if (holding.globalFreeze) {
    return "Its issuer froze the token for every holder: it moves only to and from the issuer. Peers cannot pay you on it until the issuer lifts the freeze.";
  }
  if (holding.frozen) {
    return "Its issuer froze this line: it pays only the issuer, and still receives. The node settles nothing in it until the issuer lifts the freeze.";
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
