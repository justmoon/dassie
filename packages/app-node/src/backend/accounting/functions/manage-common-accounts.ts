import { SettlementSchemeId } from "../../peer-protocol/types/settlement-scheme-id"
import { Ledger } from "../stores/ledger"

export const initializeCommonAccounts = (
  ledger: Ledger,
  settlementSchemeId: SettlementSchemeId,
) => {
  ledger.createAccount(`${settlementSchemeId}/internal/connector`)
}
