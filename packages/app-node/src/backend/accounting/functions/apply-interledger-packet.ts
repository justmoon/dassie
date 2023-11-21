import { CreateTransferParameters } from "../stores/ledger"
import { getLedgerIdFromPath } from "./get-ledger-id-from-path"

export const applyPacketPrepareToLedger = (
  sourceAccountPath: string,
  destinationAccountPath: string,
  amount: bigint,
) => {
  const transfers = []

  {
    const ledgerId = getLedgerIdFromPath(sourceAccountPath)
    const connectorPath = `${ledgerId}/internal/connector`

    const transfer: CreateTransferParameters = {
      debitAccountPath: sourceAccountPath,
      creditAccountPath: connectorPath,
      amount,
      pending: true,
    }

    transfers.push(transfer)
  }
  {
    const ledgerId = getLedgerIdFromPath(destinationAccountPath)
    const connectorPath = `${ledgerId}/internal/connector`

    const transfer: CreateTransferParameters = {
      debitAccountPath: connectorPath,
      creditAccountPath: destinationAccountPath,
      amount,
      pending: true,
    }

    transfers.push(transfer)
  }

  return transfers
}
