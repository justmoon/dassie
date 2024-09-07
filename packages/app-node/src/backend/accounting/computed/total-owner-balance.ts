import { type Reactor, createComputed } from "@dassie/lib-reactive"

import { ConvertCurrencyAmounts } from "../../exchange/functions/convert"
import type { LedgerId } from "../constants/ledgers"
import { GetCurrencyFromLedgerId } from "../functions/get-currency-from-ledger-id"
import { OwnerLedgerIdSignal } from "../signals/owner-ledger-id"
import { LedgerStore } from "../stores/ledger"
import { PostedTransfersTopic } from "../topics/posted-transfers"

export const TotalOwnerBalanceSignal = (reactor: Reactor) =>
  createComputed(reactor, () => {
    const ledger = reactor.use(LedgerStore)
    const ownerLedgerIdSignal = reactor.use(OwnerLedgerIdSignal)
    const getCurrencyFromLedgerId = reactor.use(GetCurrencyFromLedgerId)
    const convertCurrencyAmounts = reactor.use(ConvertCurrencyAmounts)

    const getLedgerBalance = (ledgerId: LedgerId) => {
      const accounts = [
        ...ledger.getAccounts(`${ledgerId}:assets/`),
        ...ledger.getAccounts(`${ledgerId}:liabilities/`),
        ...ledger.getAccounts(`${ledgerId}:contra/trust/`),
      ]

      let balance = 0n
      for (const account of accounts) {
        balance +=
          account.debitsPosted - account.creditsPosted - account.creditsPending
      }

      return balance
    }

    const calculateOwnerBalance = () => {
      const ownerLedgerId = ownerLedgerIdSignal.read()
      const ownerLedgerCurrency = getCurrencyFromLedgerId(ownerLedgerId)

      let overallBalance = 0n
      for (const ledgerId of ledger.getLedgerIds()) {
        const ledgerBalance = getLedgerBalance(ledgerId)

        if (ledgerId === ownerLedgerId) {
          overallBalance += ledgerBalance
        } else {
          const ledgerCurrency = getCurrencyFromLedgerId(ledgerId)
          overallBalance += convertCurrencyAmounts(
            ledgerCurrency,
            ownerLedgerCurrency,
            ledgerBalance,
          )
        }
      }

      return overallBalance
    }

    reactor.use(PostedTransfersTopic).on(reactor, () => {
      reactor.use(TotalOwnerBalanceSignal).write(calculateOwnerBalance())
    })

    return calculateOwnerBalance()
  })
