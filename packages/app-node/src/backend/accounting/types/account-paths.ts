import type { NodeId } from "../../peer-protocol/types/node-id"
import type { LedgerId } from "../constants/ledgers"

/**
 * A string representing a path to an account in a ledger.
 *
 * @remarks
 *
 * ### Ledger Account Structure
 *
 * ```plain
 * assets/…
 *   settlement          Assets held on the underlying ledger
 *   interledger/[peer]  Amount owed to us by the peer
 * liabilities/…
 * contra/…
 *   trust/[peer]        Represents trust extended to a peer and reduces the
 *                       interledger asset account (possibly turning it negative
 *                       and into a liability)
 * equity/…
 *   owner               Owner's equity account
 *   suspense            Suspense account (used to keep the assets/settlement
 *                       account matching the underlying ledger)
 * revenue/…
 *   fees                Fees earned by the node in the course of its operation
 *   fx                  Foreign exchange account
 * expenses/…
 *   fees                Fees paid by the node (such as node registration)
 *   fx                  Foreign exchange account
 * ```
 */
export type AccountPath =
  | AssetsOnLedgerAccount
  | AssetsInterledgerPeerAccount
  | ContraTrustPeerAccount
  | EquityOwnerAccount
  | EquitySuspenseAccount
  | RevenueFeesAccount
  | RevenueFxAccount
  | ExpensesFeesAccount
  | ExpensesFxAccount

export type AssetsOnLedgerAccount = `${LedgerId}:assets/settlement`
export type AssetsInterledgerPeerAccount =
  `${LedgerId}:assets/interledger/${NodeId}`
export type ContraTrustPeerAccount = `${LedgerId}:contra/trust/${NodeId}`
export type EquityOwnerAccount = `${LedgerId}:equity/owner`
export type EquitySuspenseAccount = `${LedgerId}:equity/suspense`
export type RevenueFeesAccount = `${LedgerId}:revenue/fees`
export type RevenueFxAccount = `${LedgerId}:revenue/fx`
export type ExpensesFeesAccount = `${LedgerId}:expenses/fees`
export type ExpensesFxAccount = `${LedgerId}:expenses/fx`
