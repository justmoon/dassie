import { Reactor } from "@dassie/lib-reactive"

import { getLedgerIdFromPath } from "../../accounting/functions/get-ledger-id-from-path"
import { AccountPath } from "../../accounting/types/account-paths"
import { GetCurrencyFromLedgerId } from "../../settlement-schemes/functions/get-currency-from-ledger-id"
import { ConvertCurrencyAmounts } from "./convert"

export const CalculateOutgoingAmount = (reactor: Reactor) => {
  const getCurrencyFromLedgerId = reactor.use(GetCurrencyFromLedgerId)
  const convertCurrencyAmounts = reactor.use(ConvertCurrencyAmounts)

  function calculateOutgoingAmount(
    sourceAccountPath: AccountPath,
    destinationAccountPath: AccountPath,
    incomingAmount: bigint,
  ) {
    const sourceLedger = getLedgerIdFromPath(sourceAccountPath)
    const destinationLedger = getLedgerIdFromPath(destinationAccountPath)

    const sourceCurrency = getCurrencyFromLedgerId(sourceLedger)
    const destinationCurrency = getCurrencyFromLedgerId(destinationLedger)

    if (sourceCurrency === destinationCurrency) {
      return incomingAmount
    }

    return convertCurrencyAmounts(
      sourceCurrency,
      destinationCurrency,
      incomingAmount,
    )
  }

  return calculateOutgoingAmount
}
