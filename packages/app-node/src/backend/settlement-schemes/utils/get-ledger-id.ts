import { LedgerId } from "../../accounting/types/ledger-id"
import { SettlementSchemeId } from "../../peer-protocol/types/settlement-scheme-id"

/**
 * Get the ledger ID for a settlement scheme.
 *
 * @remarks
 *
 * This function is just a placeholder right now. In the future, this will check the settlement module to determine the
 * ledger ID.
 */
export const getLedgerIdForSettlementScheme = (
  settlementSchemeId: SettlementSchemeId,
): LedgerId => {
  return settlementSchemeId as string as LedgerId
}
