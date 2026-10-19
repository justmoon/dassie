/**
 * The ledger's pages: account paths, and an account's transfers with its
 * balance after each.
 */

import { type Transfer, TransferState } from "@dassie/rpc/proto";

/** The ledger and the account of a path like `stub+usd:equity/owner`. */
export function splitPath(
  path: string,
): { ledger: string; account: string } | undefined {
  const colon = path.indexOf(":");
  if (colon <= 0) return undefined;
  return { ledger: path.slice(0, colon), account: path.slice(colon + 1) };
}

/** The account page's link for a path (spec/areas/wallet.md#debug-pages). */
export function accountHref(path: string): string {
  const split = splitPath(path);
  return split
    ? `/debug/ledger/${
      encodeURIComponent(split.ledger)
    }/account/${split.account}`
    : "/debug/ledger";
}

/** An account's net balance: credits less debits, posted and pending out. */
export function netBalance(account: {
  creditsPosted: bigint;
  debitsPosted: bigint;
  debitsPending: bigint;
}): bigint {
  return account.creditsPosted - account.debitsPosted - account.debitsPending;
}

/** A transfer as the account page shows it. */
export interface AccountRow {
  transfer: Transfer;
  /** Whether it credits the account. */
  credit: boolean;
  /** The other account's path. */
  counter: string;
  /** The posted balance after it; for one pending or voided, as before. */
  balance: bigint;
}

/**
 * The account's transfers, oldest first, each with the posted balance
 * after it, worked back from `posted`, the posted net balance now. Only
 * posted transfers move it.
 */
export function accountRows(
  transfers: readonly Transfer[],
  accountId: number,
  posted: bigint,
): AccountRow[] {
  const rows: AccountRow[] = [];
  let balance = posted;
  for (let index = transfers.length - 1; index >= 0; index--) {
    const transfer = transfers[index];
    const credit = transfer.credit === accountId;
    rows.push({
      transfer,
      credit,
      counter: credit ? transfer.debitPath : transfer.creditPath,
      balance,
    });
    if (transfer.state === TransferState.POSTED) {
      balance -= credit ? transfer.amount : -transfer.amount;
    }
  }
  return rows.reverse();
}

/** Merges new journal rows into what is held, by transfer ID. */
export function mergeTransfers(
  held: readonly Transfer[],
  incoming: readonly Transfer[],
  max: number,
): Transfer[] {
  const byId = new Map(held.map((transfer) => [transfer.id, transfer]));
  for (const transfer of incoming) byId.set(transfer.id, transfer);
  const merged = [...byId.values()].sort((a, b) =>
    a.id < b.id ? -1 : a.id > b.id ? 1 : 0
  );
  return merged.slice(Math.max(0, merged.length - max));
}
