import { IlpPreparePacket } from "../../ilp-connector/schemas/ilp-packet-codec"
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

  const transfer: CreateTransferParameters = {
    debitAccountPath: direction === "incoming" ? accountPath : connectorPath,
    creditAccountPath: direction === "incoming" ? connectorPath : accountPath,
    amount: packet.amount,
    pending: true,
  }

  return ledger.createTransfer(transfer)
}
