import { randomBytes } from "node:crypto"

import { NodeTableKey } from "../../peer-protocol/stores/node-table"
import { CreateTransferParameters, Ledger, Transfer } from "../stores/ledger"

export const processSettlementPrepare = (
  ledger: Ledger,
  subnetId: string,
  peerId: NodeTableKey,
  amount: bigint,
  direction: "incoming" | "outgoing"
) => {
  const peerPath = `peer/${subnetId}.${peerId}/interledger`
  const settlementPath = `peer/${subnetId}.${peerId}/settlement`

  const transfer: CreateTransferParameters = {
    key: randomBytes(16).toString("base64"),
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
