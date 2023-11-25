import { NodeId } from "../../peer-protocol/types/node-id"
import { CreateTransferParameters, Ledger } from "../stores/ledger"
import {
  AssetsInterledgerPeerAccount,
  AssetsOnLedgerAccount,
} from "../types/account-paths"
import { LedgerId } from "../types/ledger-id"

export const processSettlementPrepare = (
  ledger: Ledger,
  ledgerId: LedgerId,
  peerId: NodeId,
  amount: bigint,
  direction: "incoming" | "outgoing",
) => {
  const peerPath: AssetsInterledgerPeerAccount = `${ledgerId}:assets/interledger/${peerId}`
  const onLedgerPath: AssetsOnLedgerAccount = `${ledgerId}:assets/settlement`

  const transfer: CreateTransferParameters = {
    debitAccountPath: direction === "incoming" ? onLedgerPath : peerPath,
    creditAccountPath: direction === "incoming" ? peerPath : onLedgerPath,
    amount,
    pending: true,
  }

  return ledger.createTransfer(transfer)
}
