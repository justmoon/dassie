import type { Tagged } from "type-fest"

import type { CurrencyId } from "../../exchange/constants/currencies"

export const LEDGERS = {
  "stub+usd": {
    currency: "USD",
  },
  "xrpl+xrp": {
    currency: "XRP",
  },
  "xrpl-testnet+xrp": {
    currency: "XRP",
  },
} as const satisfies Record<string, LedgerDefinition>

export interface LedgerDefinition {
  currency: CurrencyId
}

export type LedgerId = Tagged<keyof typeof LEDGERS, "LedgerId">
