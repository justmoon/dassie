import { SetOptional, Simplify } from "type-fest"

import assert from "node:assert"

import { createLogger } from "@dassie/lib-logger"
import { Reactor } from "@dassie/lib-reactive"

import PrefixMap from "../../ilp-connector/utils/prefix-map"
import InvalidAccountFailure from "../failures/invalid-account"
import { getLedgerIdFromPath } from "../functions/get-ledger-id-from-path"
import { postedTransfersTopic } from "../topics/posted-transfers"

const logger = createLogger("das:accounting:ledger")

// Ledger Account Structure
// ------------------------
//
// internal/… -> Own accounts
// internal/connector -> Connector account
// peer/… -> Peer accounts
// peer/<nodeId> -> Accounts for a specific peer
// peer/<nodeId>/interledger -> Interledger packets
// peer/<nodeId>/settlement -> Underlying settlement account
// peer/<nodeId>/trust -> Trust extended or revoked
// owner/… -> Owner accounts
// owner/spsp/<spspAccountId> -> SPSP accounts
// owner/btp/<btpAccountId> -> BTP accounts

export interface LedgerAccount {
  path: string
  debitsPending: bigint
  debitsPosted: bigint
  creditsPending: bigint
  creditsPosted: bigint
  limit:
    | "credits_must_not_exceed_debits"
    | "debits_must_not_exceed_credits"
    | "no_limit"
}

export interface Transfer {
  key: string
  state: "pending" | "posted"
  debitAccount: string
  creditAccount: string
  amount: bigint
}

export interface CreateTransferParameters {
  key: string
  debitAccountPath: string
  creditAccountPath: string
  amount: bigint
  pending?: boolean
}

export const ledgerStore = (reactor: Reactor) => {
  const ledger = new PrefixMap<LedgerAccount>()
  const pendingTransfers = new Map<string, Transfer>()

  const postedTransfers = reactor.use(postedTransfersTopic)

  return {
    createAccount: (
      path: string,
      options: Simplify<
        Pick<SetOptional<LedgerAccount, keyof LedgerAccount>, "limit">
      > = {}
    ) => {
      const { limit } = options

      const account = {
        path,
        debitsPending: 0n,
        debitsPosted: 0n,
        creditsPending: 0n,
        creditsPosted: 0n,
        limit: limit ?? "no_limit",
      }

      logger.debug("create account", { path, limit: account.limit })

      ledger.set(path, account)
    },

    getAccount: (path: string) => ledger.get(path),

    getAccounts: (filterPrefix: string) => ledger.filterPrefix(filterPrefix),

    getPendingTransfer: (id: string) => pendingTransfers.get(id),

    createTransfer: (transferParameters: CreateTransferParameters) => {
      const { key, debitAccountPath, creditAccountPath, amount, pending } =
        transferParameters

      assert(
        creditAccountPath !== debitAccountPath,
        "transfer credit and debit accounts must be different"
      )

      assert(
        getLedgerIdFromPath(debitAccountPath) ===
          getLedgerIdFromPath(creditAccountPath),
        "transfer credit and debit accounts must be in the same ledger"
      )

      const debitAccount = ledger.get(debitAccountPath)

      if (!debitAccount) {
        return new InvalidAccountFailure("debit", debitAccountPath)
      }

      const creditAccount = ledger.get(creditAccountPath)

      if (!creditAccount) {
        return new InvalidAccountFailure("credit", creditAccountPath)
      }

      const transfer: Transfer = {
        key,
        state: pending ? "pending" : "posted",
        debitAccount: debitAccountPath,
        creditAccount: creditAccountPath,
        amount,
      }

      if (pending) {
        debitAccount.debitsPending += amount
        creditAccount.creditsPending += amount
        pendingTransfers.set(key, transfer)
      } else {
        debitAccount.debitsPosted += amount
        creditAccount.creditsPosted += amount
      }

      return transfer
    },

    postPendingTransfer: (transfer: Transfer) => {
      assert(transfer.state === "pending")

      const debitAccount = ledger.get(transfer.debitAccount)
      assert(debitAccount)

      const creditAccount = ledger.get(transfer.creditAccount)
      assert(creditAccount)

      transfer.state = "posted"

      debitAccount.debitsPending -= transfer.amount
      debitAccount.debitsPosted += transfer.amount

      creditAccount.creditsPending -= transfer.amount
      creditAccount.creditsPosted += transfer.amount

      pendingTransfers.delete(transfer.key)

      postedTransfers.emit(transfer as Transfer & { state: "posted" })
    },

    voidPendingTransfer: (transfer: Transfer) => {
      const debitAccount = ledger.get(transfer.debitAccount)
      assert(debitAccount)

      const creditAccount = ledger.get(transfer.creditAccount)
      assert(creditAccount)

      debitAccount.debitsPending -= transfer.amount

      creditAccount.creditsPending -= transfer.amount

      pendingTransfers.delete(transfer.key)
    },
  }
}

export type Ledger = ReturnType<typeof ledgerStore>
