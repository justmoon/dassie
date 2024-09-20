import { assert } from "@dassie/lib-logger"

import { accounting as logger } from "../../logger/instances"
import type { CreateTransferParameters } from "../stores/ledger"
import type {
  AccountPath,
  ExpensesFxAccount,
  RevenueFxAccount,
} from "../types/account-paths"
import { getLedgerIdFromPath } from "./get-ledger-id-from-path"

export const applyPacketPrepareToLedger = (
  sourceAccountPath: AccountPath,
  destinationAccountPath: AccountPath,
  incomingAmount: bigint,
  outgoingAmount: bigint,
): CreateTransferParameters[] => {
  const sourceLedgerId = getLedgerIdFromPath(sourceAccountPath)
  const destinationLedgerId = getLedgerIdFromPath(destinationAccountPath)

  // Same currency
  if (sourceLedgerId === destinationLedgerId) {
    assert(
      logger,
      incomingAmount === outgoingAmount,
      "incoming and outgoing amounts must match for same-currency transactions",
    )

    return [
      {
        debitAccountPath: sourceAccountPath,
        creditAccountPath: destinationAccountPath,
        amount: incomingAmount,
        pending: true,
      },
    ]
  }

  const revenueFxPath: RevenueFxAccount = `${sourceLedgerId}:revenue/fx`
  const expenseFxPath: ExpensesFxAccount = `${destinationLedgerId}:expenses/fx`

  return [
    {
      debitAccountPath: sourceAccountPath,
      creditAccountPath: revenueFxPath,
      amount: incomingAmount,
      pending: true,
    },
    {
      debitAccountPath: expenseFxPath,
      creditAccountPath: destinationAccountPath,
      amount: outgoingAmount,
      pending: true,
    },
  ]
}
