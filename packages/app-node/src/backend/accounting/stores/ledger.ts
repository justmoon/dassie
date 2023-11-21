import { SetOptional, Simplify } from "type-fest"

import assert from "node:assert"

import { Reactor } from "@dassie/lib-reactive"

import { accounting as logger } from "../../logger/instances"
import PrefixMap from "../../routing/utils/prefix-map"
import { EXCEEDS_CREDITS_FAILURE } from "../failures/exceeds-credits"
import { EXCEEDS_DEBITS_FAILURE } from "../failures/exceeds-debits"
import InvalidAccountFailure from "../failures/invalid-account"
import { getLedgerIdFromPath } from "../functions/get-ledger-id-from-path"
import { PostedTransfersTopic } from "../topics/posted-transfers"
import { AccountPath } from "../types/accounts"

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
  state: "pending" | "posted" | "voided"
  debitAccount: AccountPath
  creditAccount: AccountPath
  amount: bigint
}

export interface CreateTransferParameters {
  debitAccountPath: AccountPath
  creditAccountPath: AccountPath
  amount: bigint
  pending?: boolean
}

export const LedgerStore = (reactor: Reactor) => {
  const ledger = new PrefixMap<LedgerAccount>()
  const pendingTransfers = new Set<Transfer>()

  const postedTransfers = reactor.use(PostedTransfersTopic)

  return {
    createAccount: (
      path: AccountPath,
      options: Simplify<
        Pick<SetOptional<LedgerAccount, keyof LedgerAccount>, "limit">
      > = {},
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

    getAccount: (path: AccountPath) => ledger.get(path),

    getAccounts: (filterPrefix: string) => ledger.filterPrefix(filterPrefix),

    getPendingTransfers: () => [...pendingTransfers],

    createTransfer: (transferParameters: CreateTransferParameters) => {
      const { debitAccountPath, creditAccountPath, amount, pending } =
        transferParameters

      assert(
        creditAccountPath !== debitAccountPath,
        "transfer credit and debit accounts must be different",
      )

      assert(
        getLedgerIdFromPath(debitAccountPath) ===
          getLedgerIdFromPath(creditAccountPath),
        "transfer credit and debit accounts must be in the same ledger",
      )

      const debitAccount = ledger.get(debitAccountPath)

      if (!debitAccount) {
        return new InvalidAccountFailure("debit", debitAccountPath)
      }

      if (
        debitAccount.limit === "debits_must_not_exceed_credits" &&
        debitAccount.debitsPosted + debitAccount.debitsPending + amount >
          debitAccount.creditsPosted
      ) {
        return EXCEEDS_DEBITS_FAILURE
      }

      const creditAccount = ledger.get(creditAccountPath)

      if (!creditAccount) {
        return new InvalidAccountFailure("credit", creditAccountPath)
      }

      if (
        creditAccount.limit === "credits_must_not_exceed_debits" &&
        creditAccount.creditsPosted + creditAccount.creditsPending + amount >
          creditAccount.debitsPosted
      ) {
        return EXCEEDS_CREDITS_FAILURE
      }

      const transfer: Transfer = {
        state: pending ? "pending" : "posted",
        debitAccount: debitAccountPath,
        creditAccount: creditAccountPath,
        amount,
      }

      if (pending) {
        debitAccount.debitsPending += amount
        creditAccount.creditsPending += amount
        pendingTransfers.add(transfer)
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

      pendingTransfers.delete(transfer)

      postedTransfers.emit(transfer as Transfer & { state: "posted" })
    },

    voidPendingTransfer: (transfer: Transfer) => {
      assert(transfer.state === "pending")

      const debitAccount = ledger.get(transfer.debitAccount)
      assert(debitAccount)

      const creditAccount = ledger.get(transfer.creditAccount)
      assert(creditAccount)

      transfer.state = "voided"

      debitAccount.debitsPending -= transfer.amount

      creditAccount.creditsPending -= transfer.amount

      pendingTransfers.delete(transfer)
    },
  }
}

export type Ledger = ReturnType<typeof LedgerStore>
