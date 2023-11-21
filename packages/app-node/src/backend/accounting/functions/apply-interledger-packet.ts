import { CreateTransferParameters } from "../stores/ledger"
import { AccountPath, ConnectorAccount } from "../types/accounts"
import { getLedgerIdFromPath } from "./get-ledger-id-from-path"

export const applyPacketPrepareToLedger = (
  sourceAccountPath: AccountPath,
  destinationAccountPath: AccountPath,
  amount: bigint,
) => {
  const transfers = []

  {
    const ledgerId = getLedgerIdFromPath(sourceAccountPath)
    const connectorPath: ConnectorAccount = `${ledgerId}:internal/connector`

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
    const connectorPath: ConnectorAccount = `${ledgerId}:internal/connector`

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
