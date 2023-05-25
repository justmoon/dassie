import { createComputed } from "@dassie/lib-reactive"

import { ledgerStore } from "../stores/ledger"
import { postedTransfersTopic } from "../topics/posted-transfers"

export const totalOwnerBalanceComputed = () =>
  createComputed((sig) => {
    const ledger = sig.use(ledgerStore)

    let balance = 0n
    for (const account of ledger.getAccounts("builtin/owner/")) {
      balance += account.creditsPosted - account.debitsPosted
    }

    sig.use(postedTransfersTopic).on(sig.reactor, (transfer) => {
      let newBalance = balance
      if (transfer.creditAccount.startsWith("builtin/owner/")) {
        newBalance += transfer.amount
      }

      if (transfer.debitAccount.startsWith("builtin/owner/")) {
        newBalance -= transfer.amount
      }

      if (newBalance !== balance) {
        sig.use(totalOwnerBalanceComputed).write(newBalance)
        balance = newBalance
      }
    })

    return balance
  })
