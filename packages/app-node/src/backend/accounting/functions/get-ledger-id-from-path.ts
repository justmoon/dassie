import { accounting as logger } from "../../logger/instances"
import { LedgerId } from "../constants/ledgers"
import { AccountPath } from "../types/account-paths"

export const getLedgerIdFromPath = (path: AccountPath): LedgerId => {
  const colonPosition = path.indexOf(":")

  logger.assert(colonPosition !== -1, "account paths must contain a colon")

  return path.slice(0, colonPosition) as LedgerId
}
