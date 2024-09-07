import type { Reactor } from "@dassie/lib-reactive"

import { GetCurrencyFromLedgerId } from "../../accounting/functions/get-currency-from-ledger-id"
import { getLedgerIdFromPath } from "../../accounting/functions/get-ledger-id-from-path"
import type { AccountPath } from "../../accounting/types/account-paths"
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
