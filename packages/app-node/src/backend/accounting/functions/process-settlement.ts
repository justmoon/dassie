import { NodeId } from "../../peer-protocol/types/node-id"
import { CreateTransferParameters, Ledger } from "../stores/ledger"

export const processSettlementPrepare = (
  ledger: Ledger,
  settlementSchemeId: string,
  peerId: NodeId,
  amount: bigint,
  direction: "incoming" | "outgoing",
) => {
  const peerPath = `${settlementSchemeId}/peer/${peerId}/interledger`
  const settlementPath = `${settlementSchemeId}/peer/${peerId}/settlement`

  const transfer: CreateTransferParameters = {
    debitAccountPath: direction === "incoming" ? settlementPath : peerPath,
    creditAccountPath: direction === "incoming" ? peerPath : settlementPath,
    amount,
    pending: true,
  }

  return ledger.createTransfer(transfer)
}
