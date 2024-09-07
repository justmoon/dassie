import { assert } from "@dassie/lib-logger"

import { accounting as logger } from "../../logger/instances"
import type { LedgerId } from "../constants/ledgers"
import type { AccountPath } from "../types/account-paths"

export const getLedgerIdFromPath = (path: AccountPath): LedgerId => {
  const colonPosition = path.indexOf(":")

  assert(logger, colonPosition !== -1, "account paths must contain a colon")

  return path.slice(0, colonPosition) as LedgerId
}
