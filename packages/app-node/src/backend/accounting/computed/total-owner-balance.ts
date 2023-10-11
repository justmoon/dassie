import { Reactor, createComputed } from "@dassie/lib-reactive"

import { ledgerStore } from "../stores/ledger"
import { PostedTransfersTopic } from "../topics/posted-transfers"

export const TotalOwnerBalanceSignal = (reactor: Reactor) =>
  createComputed(reactor.lifecycle, () => {
    const ledger = reactor.use(ledgerStore)

    let balance = 0n
    for (const account of ledger.getAccounts("builtin/owner/")) {
      balance += account.creditsPosted - account.debitsPosted
    }

    reactor.use(PostedTransfersTopic).on(reactor.lifecycle, (transfer) => {
      let newBalance = balance
      if (transfer.creditAccount.startsWith("builtin/owner/")) {
        newBalance += transfer.amount
      }

      if (transfer.debitAccount.startsWith("builtin/owner/")) {
        newBalance -= transfer.amount
      }

      if (newBalance !== balance) {
        reactor.use(TotalOwnerBalanceSignal).write(newBalance)
        balance = newBalance
      }
    })

    return balance
  })
