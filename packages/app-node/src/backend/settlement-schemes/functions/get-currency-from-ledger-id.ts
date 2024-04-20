import type { Reactor } from "@dassie/lib-reactive"

import { LedgerId } from "../../accounting/types/ledger-id"
import { CurrencyDescription } from "../../exchange/load-exchange-rates"
import { settlement as logger } from "../../logger/instances"
import { LoadedSettlementModulesStore } from "../stores/loaded-settlement-modules"

export const GetCurrencyFromLedgerId = (reactor: Reactor) => {
  const loadedSettlementModulesStore = reactor.use(LoadedSettlementModulesStore)
  const ledgerIdToCurrency = new Map<LedgerId, CurrencyDescription>()

  for (const module of loadedSettlementModulesStore.read().values()) {
    ledgerIdToCurrency.set(module.ledger.id, module.ledger.currency)
  }

  loadedSettlementModulesStore.changes.on(reactor, ([action, change]) => {
    if (action === "loadModule") {
      const [, module] = change
      ledgerIdToCurrency.set(module.ledger.id, module.ledger.currency)
    }
  })

  function getCurrencyFromLedgerId(ledgerId: LedgerId) {
    const currency = ledgerIdToCurrency.get(ledgerId)

    if (!currency) {
      logger.error("failed to map ledger ID to currency", { ledgerId })
      throw new Error("Failed to map ledger ID to currency")
    }

    return currency
  }

  return getCurrencyFromLedgerId
}
