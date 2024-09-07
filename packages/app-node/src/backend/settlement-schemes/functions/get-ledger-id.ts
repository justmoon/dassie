import type { Reactor } from "@dassie/lib-reactive"

import type { LedgerId } from "../../accounting/constants/ledgers"
import type { SettlementSchemeId } from "../../peer-protocol/types/settlement-scheme-id"
import { LoadedSettlementModulesStore } from "../stores/loaded-settlement-modules"

/**
 * Get the ledger ID for a settlement scheme.
 *
 * @remarks
 *
 * This function is just a placeholder right now. In the future, this will check the settlement module to determine the
 * ledger ID.
 */
export const GetLedgerIdForSettlementScheme = (reactor: Reactor) => {
  const loadedSettlementModulesStore = reactor.use(LoadedSettlementModulesStore)

  return function getLedgerIdForSettlementScheme(
    settlementSchemeId: SettlementSchemeId,
  ): LedgerId {
    const module = loadedSettlementModulesStore.read().get(settlementSchemeId)

    if (!module) {
      throw new Error(`Unknown settlement scheme '${settlementSchemeId}'`)
    }

    return module.ledger
  }
}
