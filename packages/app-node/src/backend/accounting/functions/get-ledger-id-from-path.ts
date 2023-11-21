import assert from "node:assert"

import { AccountPath } from "../types/accounts"
import { LedgerId } from "../types/ledger-id"

export const getLedgerIdFromPath = (path: AccountPath): LedgerId => {
  const colonPosition = path.indexOf(":")

  assert(colonPosition !== -1, "account paths must contain a colon")

  return path.slice(0, colonPosition) as LedgerId
}
