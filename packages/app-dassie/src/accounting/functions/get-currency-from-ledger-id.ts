import type { UnwrapTagged } from "type-fest"

import { CURRENCIES } from "../../exchange/constants/currencies"
import { settlement as logger } from "../../logger/instances"
import { LEDGERS, type LedgerId } from "../constants/ledgers"

function getCurrencyFromLedgerId(ledgerId: LedgerId) {
  const ledger = LEDGERS[ledgerId as UnwrapTagged<LedgerId>]

  // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition
  if (!ledger) {
    logger.error("failed to map ledger ID to currency - unknown ledger", {
      ledger: ledgerId,
    })
    throw new Error("Failed to map ledger ID to currency")
  }

  const currency = CURRENCIES[ledger.currency]

  // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition
  if (!currency) {
    logger.error("failed to map ledger ID to currency - unknown currency", {
      ledger: ledgerId,
      currency: ledger.currency,
    })
    throw new Error("Failed to map ledger ID to currency")
  }

  return currency
}

export const GetCurrencyFromLedgerId = () => getCurrencyFromLedgerId
