import type { SettlementSchemeId } from "@dassie/app-dassie/src/peer-protocol/types/settlement-scheme-id"

import type { CurrencySpecification } from "../types/currency"
import { USD_SPECIFICATION, XRP_SPECIFICATION } from "./currency"

export const SCHEME_NAME_MAP: Record<SettlementSchemeId, string> = {
  stub: "Stub",
  xrpl: "XRP Ledger",
  "xrpl-testnet": "XRP Ledger Testnet",
}

export const SCHEME_CURRENCY_MAP: Record<
  SettlementSchemeId,
  CurrencySpecification
> = {
  stub: USD_SPECIFICATION,
  xrpl: XRP_SPECIFICATION,
  "xrpl-testnet": XRP_SPECIFICATION,
}
