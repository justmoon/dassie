import type { NodeId } from "../../peer-protocol/types/node-id"
import type { LedgerId } from "../constants/ledgers"
import type { CreateTransferParameters, Ledger } from "../stores/ledger"
import type {
  AssetsInterledgerPeerAccount,
  AssetsOnLedgerAccount,
} from "../types/account-paths"

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
