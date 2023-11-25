import { Ledger } from "../stores/ledger"
import { LedgerId } from "../types/ledger-id"

export const initializeCommonAccounts = (
  ledger: Ledger,
  ledgerId: LedgerId,
) => {
  ledger.createAccount(`${ledgerId}:assets/settlement`)
  ledger.createAccount(`${ledgerId}:equity/owner`)
  ledger.createAccount(`${ledgerId}:equity/suspense`)
  ledger.createAccount(`${ledgerId}:revenue/fx`)
  ledger.createAccount(`${ledgerId}:revenue/fees`)
  ledger.createAccount(`${ledgerId}:expenses/fx`)
  ledger.createAccount(`${ledgerId}:expenses/fees`)
}
