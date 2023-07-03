import { NodeId } from "../../peer-protocol/types/node-id"
import { CreateTransferParameters, Ledger } from "../stores/ledger"

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
