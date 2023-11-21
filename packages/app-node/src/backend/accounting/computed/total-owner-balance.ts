import { Reactor, createComputed } from "@dassie/lib-reactive"

import { OwnerLedgerIdSignal } from "../signals/owner-ledger-id"
import { LedgerStore } from "../stores/ledger"
import { PostedTransfersTopic } from "../topics/posted-transfers"

export const TotalOwnerBalanceSignal = (reactor: Reactor) =>
  createComputed(reactor, () => {
    const ledger = reactor.use(LedgerStore)
    const ownerLedgerIdSignal = reactor.use(OwnerLedgerIdSignal)

    let balance = 0n
    for (const account of ledger.getAccounts(
      `${ownerLedgerIdSignal.read()}:owner/`,
    )) {
      balance += account.creditsPosted - account.debitsPosted
    }

    reactor.use(PostedTransfersTopic).on(reactor, (transfer) => {
      const ownerAccountPrefix = `${ownerLedgerIdSignal.read()}:owner/`

      let newBalance = balance
      if (transfer.creditAccount.startsWith(ownerAccountPrefix)) {
        newBalance += transfer.amount
      }

      if (transfer.debitAccount.startsWith(ownerAccountPrefix)) {
        newBalance -= transfer.amount
      }

      if (newBalance !== balance) {
        reactor.use(TotalOwnerBalanceSignal).write(newBalance)
        balance = newBalance
      }
    })

    return balance
  })
