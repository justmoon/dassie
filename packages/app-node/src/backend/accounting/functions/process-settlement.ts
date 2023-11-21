import { NodeId } from "../../peer-protocol/types/node-id"
import { CreateTransferParameters, Ledger } from "../stores/ledger"
import {
  PeerInterledgerAccount,
  PeerSettlementAccount,
} from "../types/accounts"
import { LedgerId } from "../types/ledger-id"

export const processSettlementPrepare = (
  ledger: Ledger,
  ledgerId: LedgerId,
  peerId: NodeId,
  amount: bigint,
  direction: "incoming" | "outgoing",
) => {
  const peerPath: PeerInterledgerAccount = `${ledgerId}:peer/${peerId}/interledger`
  const settlementPath: PeerSettlementAccount = `${ledgerId}:peer/${peerId}/settlement`

  const transfer: CreateTransferParameters = {
    debitAccountPath: direction === "incoming" ? settlementPath : peerPath,
    creditAccountPath: direction === "incoming" ? peerPath : settlementPath,
    amount,
    pending: true,
  }

  return ledger.createTransfer(transfer)
}
