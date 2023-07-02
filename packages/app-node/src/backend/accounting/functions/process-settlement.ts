import { NodeId } from "../../peer-protocol/types/node-id"
import { CreateTransferParameters, Ledger, Transfer } from "../stores/ledger"

export const processSettlementPrepare = (
  ledger: Ledger,
  subnetId: string,
  peerId: NodeId,
  amount: bigint,
  direction: "incoming" | "outgoing"
) => {
  const peerPath = `${subnetId}/peer/${peerId}/interledger`
  const settlementPath = `${subnetId}/peer/${peerId}/settlement`

  const transfer: CreateTransferParameters = {
    debitAccountPath: direction === "incoming" ? settlementPath : peerPath,
    creditAccountPath: direction === "incoming" ? peerPath : settlementPath,
    amount,
    pending: true,
  }

  return ledger.createTransfer(transfer)
}

export const processSettlementResult = (
  ledger: Ledger,
  transfer: Transfer,
  result: "fulfill" | "reject"
) => {
  if (result === "fulfill") {
    ledger.postPendingTransfer(transfer)
  } else {
    ledger.voidPendingTransfer(transfer)
  }
}
