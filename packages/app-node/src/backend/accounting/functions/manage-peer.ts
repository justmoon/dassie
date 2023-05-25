import assert from "node:assert"

import { NodeId } from "../../peer-protocol/types/node-id"
import { SubnetId } from "../../peer-protocol/types/subnet-id"
import { Ledger } from "../stores/ledger"

const DEFAULT_CREDIT = 1_000_000_000n

export const initializePeer = (
  ledger: Ledger,
  subnetId: SubnetId,
  peerId: NodeId
) => {
  ledger.createAccount(`peer/${subnetId}.${peerId}/interledger`, {
    limit: "debits_must_not_exceed_credits",
  })
  ledger.createAccount(`peer/${subnetId}.${peerId}/settlement`)
  ledger.createAccount(`peer/${subnetId}.${peerId}/trust`, {
    limit: "credits_must_not_exceed_debits",
  })

  // Extend initial trust limit
  ledger.createTransfer({
    key: `${peerId}/initialTrust`,
    debitAccountPath: `peer/${subnetId}.${peerId}/trust`,
    creditAccountPath: `peer/${subnetId}.${peerId}/interledger`,
    amount: DEFAULT_CREDIT,
  })
}

export const cleanupPeer = (ledger: Ledger, peerId: NodeId) => {
  // TODO: void all pending transfers?
  assert(ledger)
  assert(peerId)
}
