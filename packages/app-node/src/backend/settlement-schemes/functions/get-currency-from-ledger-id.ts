import { LedgerId } from "../../accounting/types/ledger-id"
import { CurrencyDescription } from "../../exchange/load-exchange-rates"
import modules from "../modules"

export const GetCurrencyFromLedgerId = () => {
  const ledgerIdToCurrency = new Map<LedgerId, CurrencyDescription>()

  for (const module of Object.values(modules)) {
    ledgerIdToCurrency.set(module.ledger.id, module.ledger.currency)
  }

  function getCurrencyFromLedgerId(ledgerId: LedgerId) {
    const currency = ledgerIdToCurrency.get(ledgerId)

    if (!currency) {
      throw new Error("Failed to map ledger ID to currency")
    }

    return currency
  }

  return getCurrencyFromLedgerId
}
