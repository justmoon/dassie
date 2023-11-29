import { accounting as logger } from "../../logger/instances"
import { AccountPath } from "../types/account-paths"
import { LedgerId } from "../types/ledger-id"

export const getLedgerIdFromPath = (path: AccountPath): LedgerId => {
  const colonPosition = path.indexOf(":")

  logger.assert(colonPosition !== -1, "account paths must contain a colon")

  return path.slice(0, colonPosition) as LedgerId
}
