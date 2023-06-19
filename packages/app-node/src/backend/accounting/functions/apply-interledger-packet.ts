import { IlpPreparePacket } from "../../ilp-connector/ilp-packet-codec"
import { CreateTransferParameters, Ledger } from "../stores/ledger"
import { getLedgerIdFromPath } from "./get-ledger-id-from-path"

export const applyPacketPrepareToLedger = (
  ledger: Ledger,
  accountPath: string,
  packet: IlpPreparePacket,
  direction: "incoming" | "outgoing"
) => {
  const ledgerId = getLedgerIdFromPath(accountPath)
  const connectorPath = `${ledgerId}/internal/connector`

  const base64Condition = Buffer.from(packet.executionCondition).toString(
    "base64"
  )
  const transfer: CreateTransferParameters = {
    key: `${accountPath};${base64Condition}`,
    debitAccountPath: direction === "incoming" ? accountPath : connectorPath,
    creditAccountPath: direction === "incoming" ? connectorPath : accountPath,
    amount: packet.amount,
    pending: true,
  }

  return ledger.createTransfer(transfer)
}

export const applyPacketResultToLedger = (
  ledger: Ledger,
  accountPath: string,
  packet: IlpPreparePacket,
  result: "fulfill" | "reject"
) => {
  const base64Condition = Buffer.from(packet.executionCondition).toString(
    "base64"
  )
  const transfer = ledger.getPendingTransfer(
    `${accountPath};${base64Condition}`
  )

  if (!transfer) {
    throw new Error(`No pending transfer for condition ${base64Condition}`)
  }

  if (result === "fulfill") {
    ledger.postPendingTransfer(transfer)
  } else {
    ledger.voidPendingTransfer(transfer)
  }
}
