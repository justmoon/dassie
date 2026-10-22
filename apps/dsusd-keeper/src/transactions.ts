// The transaction each step submits, and which account signs it. The fee,
// sequence and last ledger are filled in when it is submitted.

import type { Step } from "./plan.ts";
import { LINE_LIMIT } from "./state.ts";
import { DSUSD_CURRENCY, dsusdValue } from "./units.ts";

/** The pool's trading fee, in 1/100,000: 0.3%, which market makers pay on
 * each swap and which keeps a swap against a stale price from paying. */
export const TRADING_FEE_UNITS = 300;

/** AccountSet: DefaultRipple (asfDefaultRipple). */
const ASF_DEFAULT_RIPPLE = 8;
/** TrustSet: NoRipple on the sender's side (tfSetNoRipple). */
const TF_SET_NO_RIPPLE = 0x0002_0000;
/** AMMDeposit and AMMWithdraw: one asset (tfSingleAsset), both at the
 * pool's ratio (tfTwoAsset), and both into an emptied pool
 * (tfTwoAssetIfEmpty). */
export const TF_SINGLE_ASSET = 0x0008_0000;
export const TF_TWO_ASSET = 0x0010_0000;
export const TF_TWO_ASSET_IF_EMPTY = 0x0080_0000;

export type Addresses = { issuer: string; keeper: string };

/** A transaction as the ledger's JSON, before it is filled in. */
export type Transaction = Record<string, unknown> & {
  TransactionType: string;
  Account: string;
};

/** What a step submits: the transaction and who signs it. */
export type Submission = { signer: "issuer" | "keeper"; tx: Transaction };

/** The transaction `step` submits, or null for a step that submits none
 * (the faucet, done, stuck). */
export function transactionOf(
  step: Step,
  addresses: Addresses,
): Submission | null {
  const { issuer, keeper } = addresses;
  const dsusd = (micros: bigint) => ({
    currency: DSUSD_CURRENCY,
    issuer,
    value: dsusdValue(micros),
  });
  const pair = {
    Asset: { currency: "XRP" },
    Asset2: { currency: DSUSD_CURRENCY, issuer },
  };
  switch (step.kind) {
    case "default-ripple":
      return {
        signer: "issuer",
        tx: {
          TransactionType: "AccountSet",
          Account: issuer,
          SetFlag: ASF_DEFAULT_RIPPLE,
        },
      };
    case "trust-line":
      return {
        signer: "keeper",
        tx: {
          TransactionType: "TrustSet",
          Account: keeper,
          LimitAmount: { currency: DSUSD_CURRENCY, issuer, value: LINE_LIMIT },
          Flags: TF_SET_NO_RIPPLE,
        },
      };
    case "mint":
      return {
        signer: "issuer",
        tx: {
          TransactionType: "Payment",
          Account: issuer,
          Destination: keeper,
          Amount: dsusd(step.micros),
        },
      };
    case "burn":
      return {
        signer: "keeper",
        tx: {
          TransactionType: "Payment",
          Account: keeper,
          Destination: issuer,
          Amount: dsusd(step.micros),
        },
      };
    case "create-pool":
      return {
        signer: "keeper",
        tx: {
          TransactionType: "AMMCreate",
          Account: keeper,
          Amount: step.drops.toString(),
          Amount2: dsusd(step.micros),
          TradingFee: TRADING_FEE_UNITS,
        },
      };
    case "refill-pool":
      return {
        signer: "keeper",
        tx: {
          TransactionType: "AMMDeposit",
          Account: keeper,
          ...pair,
          Amount: step.drops.toString(),
          Amount2: dsusd(step.micros),
          Flags: TF_TWO_ASSET_IF_EMPTY,
        },
      };
    case "raise":
      return {
        signer: "keeper",
        tx: {
          TransactionType: "AMMDeposit",
          Account: keeper,
          ...pair,
          Amount: dsusd(step.micros),
          Flags: TF_SINGLE_ASSET,
        },
      };
    case "lower":
      return {
        signer: "keeper",
        tx: {
          TransactionType: "AMMWithdraw",
          Account: keeper,
          ...pair,
          Amount: dsusd(step.micros),
          Flags: TF_SINGLE_ASSET,
        },
      };
    case "deepen":
      return {
        signer: "keeper",
        tx: {
          TransactionType: "AMMDeposit",
          Account: keeper,
          ...pair,
          Amount: step.drops.toString(),
          Amount2: dsusd(step.micros),
          Flags: TF_TWO_ASSET,
        },
      };
    case "fund":
    case "done":
    case "stuck":
      return null;
  }
}
