/**
 * A dollar token's peg guard, for people (spec/areas/wallet.md#pages,
 * spec/areas/xrpl.md#the-peg-guard): whether the node paused the ledger's
 * scheme, and why, and the token's price at the guard's last reading. The
 * node decides; the wallet only says what it reports.
 */

import { type LedgerPeg, PegPause } from "@dassie/rpc/proto";

/** A millionth of a dollar's decimals. */
const PPM_DECIMALS = 6;
/** The decimals a price is shown with. */
const SHOWN_DECIMALS = 4;

/** What the owner is told of a paused scheme. */
export interface PegNotice {
  readonly title: string;
  readonly text: string;
}

/**
 * A price in millionths of a dollar as dollars, cut to four decimals,
 * never rounded, so a price is never shown nearer a dollar than it is:
 * `950000` is `$0.9500`.
 */
export function priceText(ppm: bigint): string {
  const unit = 10n ** BigInt(PPM_DECIMALS);
  const cut = 10n ** BigInt(PPM_DECIMALS - SHOWN_DECIMALS);
  const fraction = (ppm % unit) / cut;
  return `$${ppm / unit}.${fraction.toString().padStart(SHOWN_DECIMALS, "0")}`;
}

/**
 * What to say of `peg`, the guard of a scheme in `token` whose price comes
 * from the AMM pool `pool`, such as "XRP/RLUSD": nothing while it runs.
 */
export function pegNotice(
  peg: LedgerPeg,
  token: string,
  pool: string,
): PegNotice | undefined {
  const effect =
    `The node settles nothing in ${token}, asks no peer to pay in it and converts nothing to or from it; what arrives on its line is still booked.`;
  switch (peg.paused) {
    case PegPause.OFF_PEG: {
      const price = peg.pricePpm === undefined
        ? ""
        : ` at ${priceText(peg.pricePpm)}`;
      return {
        title: `Paused: ${token} is off its dollar peg`,
        text:
          `Its ${pool} AMM pool prices it${price}, too far from a dollar for too long. ${effect} It runs again once the pool has priced it near a dollar for long enough.`,
      };
    }
    case PegPause.STALE:
      return {
        title: `Paused: the price of ${token} cannot be read`,
        text:
          `The node has not read ${token}'s price from its ${pool} AMM pool for too long: the pool could not be read or holds too little, or the node's XRP rate is stale. ${effect} It runs again with the next price near a dollar.`,
      };
    default:
      return undefined;
  }
}

/** The token's last price, for a guard that read one: `$1.0002, as of
 * ledger 123`. */
export function lastPriceText(peg: LedgerPeg): string | undefined {
  if (peg.pricePpm === undefined) return undefined;
  return `${priceText(peg.pricePpm)}, as of ledger ${peg.ledgerIndex}`;
}
