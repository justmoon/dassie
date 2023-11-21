import { Ledger } from "../stores/ledger"
import { LedgerId } from "../types/ledger-id"

export const initializeCommonAccounts = (
  ledger: Ledger,
  ledgerId: LedgerId,
) => {
  ledger.createAccount(`${ledgerId}:internal/connector`)
}
