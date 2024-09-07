import type { SetOptional, Simplify } from "type-fest"

import { assert } from "@dassie/lib-logger"
import type { Reactor } from "@dassie/lib-reactive"

import { Database } from "../../database/open-database"
import { accounting as logger } from "../../logger/instances"
import PrefixMap from "../../routing/utils/prefix-map"
import type { LedgerId } from "../constants/ledgers"
import { EXCEEDS_CREDITS_FAILURE } from "../failures/exceeds-credits"
import { EXCEEDS_DEBITS_FAILURE } from "../failures/exceeds-debits"
import InvalidAccountFailure from "../failures/invalid-account"
import { getLedgerIdFromPath } from "../functions/get-ledger-id-from-path"
import { PendingTransfersTopic } from "../topics/pending-transfers"
import { PostedTransfersTopic } from "../topics/posted-transfers"
import { VoidedTransfersTopic } from "../topics/voided-transfers"
import type { AccountPath } from "../types/account-paths"

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
  immediate: boolean
}

export interface CreateTransferParameters {
  debitAccountPath: AccountPath
  creditAccountPath: AccountPath
  amount: bigint
  pending?: boolean
}

export const LedgerStore = (reactor: Reactor) => {
  const ledger = new PrefixMap<AccountPath, LedgerAccount>()
  const pendingTransfers = new Set<Transfer>()

  const database = reactor.use(Database)
  const postedTransfersTopic = reactor.use(PostedTransfersTopic)
  const pendingTransfersTopic = reactor.use(PendingTransfersTopic)
  const voidedTransfersTopic = reactor.use(VoidedTransfersTopic)

  function persistPostedTransfer({
    debitAccount,
    creditAccount,
    amount,
  }: Transfer) {
    logger.info("persist posted transfer", {
      debitAccount,
      creditAccount,
      amount,
    })
    database.transaction(() => {
      database.executeSync(
        database.kysely
          .updateTable("accounts")
          .where("path", "=", debitAccount)
          .set((eb) => ({
            debits_posted: eb("debits_posted", "+", amount),
          }))
          .compile(),
      )

      database.executeSync(
        database.kysely
          .updateTable("accounts")
          .where("path", "=", creditAccount)
          .set((eb) => ({
            credits_posted: eb("credits_posted", "+", amount),
          }))
          .compile(),
      )
    })
  }

  return {
    createAccount: (
      path: AccountPath,
      options: Simplify<
        Pick<SetOptional<LedgerAccount, keyof LedgerAccount>, "limit">
      > = {},
    ) => {
      const { limit } = options

      const databaseRow = database.tables.accounts.selectFirst({ path })

      const account = {
        path,
        debitsPending: 0n,
        debitsPosted: databaseRow?.debits_posted ?? 0n,
        creditsPending: 0n,
        creditsPosted: databaseRow?.credits_posted ?? 0n,
        limit: limit ?? "no_limit",
      }

      if (!databaseRow) {
        database.tables.accounts.insertOne({ path })
      }

      logger.debug?.(databaseRow ? "load account" : "create account", {
        path,
        limit: account.limit,
        debits: account.debitsPosted,
        credits: account.creditsPosted,
      })

      ledger.set(path, account)
    },

    getLedgerIds: (): LedgerId[] => {
      const ledgerIds = new Set<LedgerId>()

      for (const accountPath of ledger.keys()) {
        ledgerIds.add(getLedgerIdFromPath(accountPath))
      }

      return [...ledgerIds]
    },

    getAccount: (path: AccountPath) => ledger.get(path),

    getAccounts: (filterPrefix: string) => ledger.filterPrefix(filterPrefix),

    getPendingTransfers: () => [...pendingTransfers],

    createTransfer: (transferParameters: CreateTransferParameters) => {
      const { debitAccountPath, creditAccountPath, amount, pending } =
        transferParameters

      assert(
        logger,
        creditAccountPath !== debitAccountPath,
        "transfer credit and debit accounts must be different",
      )

      assert(
        logger,
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
        immediate: !pending,
      }

      if (pending) {
        debitAccount.debitsPending += amount
        creditAccount.creditsPending += amount
        pendingTransfers.add(transfer)

        pendingTransfersTopic.emit(transfer as Transfer & { state: "pending" })
      } else {
        debitAccount.debitsPosted += amount
        creditAccount.creditsPosted += amount

        postedTransfersTopic.emit(transfer as Transfer & { state: "posted" })

        persistPostedTransfer(transfer)
      }

      return transfer
    },

    postPendingTransfer: (transfer: Transfer) => {
      assert(logger, transfer.state === "pending", "transfer must be pending")

      const debitAccount = ledger.get(transfer.debitAccount)
      assert(logger, !!debitAccount, "debit account must exist")

      const creditAccount = ledger.get(transfer.creditAccount)
      assert(logger, !!creditAccount, "credit account must exist")

      transfer.state = "posted"

      debitAccount.debitsPending -= transfer.amount
      debitAccount.debitsPosted += transfer.amount

      creditAccount.creditsPending -= transfer.amount
      creditAccount.creditsPosted += transfer.amount

      pendingTransfers.delete(transfer)

      persistPostedTransfer(transfer)

      postedTransfersTopic.emit(transfer as Transfer & { state: "posted" })
    },

    voidPendingTransfer: (transfer: Transfer) => {
      assert(logger, transfer.state === "pending", "transfer must be pending")

      const debitAccount = ledger.get(transfer.debitAccount)
      assert(logger, !!debitAccount, "debit account must exist")

      const creditAccount = ledger.get(transfer.creditAccount)
      assert(logger, !!creditAccount, "credit account must exist")

      transfer.state = "voided"

      debitAccount.debitsPending -= transfer.amount

      creditAccount.creditsPending -= transfer.amount

      pendingTransfers.delete(transfer)

      voidedTransfersTopic.emit(transfer as Transfer & { state: "voided" })
    },
  }
}

export type Ledger = ReturnType<typeof LedgerStore>
