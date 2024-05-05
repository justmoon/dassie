import type { Tagged } from "type-fest"

import { LEDGERS } from "../constants/ledgers"

export function castLedgerId<T extends keyof typeof LEDGERS>(
  id: T,
): Tagged<T, "LedgerId"> {
  if (!LEDGERS[id]) {
    throw new Error(`Unknown ledger ID: ${id}`)
  }

  return id as Tagged<T, "LedgerId">
}
