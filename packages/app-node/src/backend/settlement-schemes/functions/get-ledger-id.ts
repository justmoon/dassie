import { LedgerId } from "../../accounting/types/ledger-id"
import { SettlementSchemeId } from "../../peer-protocol/types/settlement-scheme-id"
import modules from "../modules"

/**
 * Get the ledger ID for a settlement scheme.
 *
 * @remarks
 *
 * This function is just a placeholder right now. In the future, this will check the settlement module to determine the
 * ledger ID.
 */
export const GetLedgerIdForSettlementScheme = () => {
  return (settlementSchemeId: SettlementSchemeId): LedgerId => {
    const module = modules[settlementSchemeId]

    if (!module) {
      throw new Error(`Unknown settlement scheme '${settlementSchemeId}'`)
    }

    return module.ledger.id
  }
}
