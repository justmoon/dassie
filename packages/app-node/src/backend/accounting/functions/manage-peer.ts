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
  ledger.createAccount(`${subnetId}/peer/${peerId}/interledger`, {
    limit: "debits_must_not_exceed_credits",
  })
  ledger.createAccount(`${subnetId}/peer/${peerId}/settlement`)
  ledger.createAccount(`${subnetId}/peer/${peerId}/trust`, {
    limit: "credits_must_not_exceed_debits",
  })

  // Extend initial trust limit
  ledger.createTransfer({
    key: `${peerId}/initialTrust`,
    debitAccountPath: `${subnetId}/peer/${peerId}/trust`,
    creditAccountPath: `${subnetId}/peer/${peerId}/interledger`,
    amount: DEFAULT_CREDIT,
  })
}

export const cleanupPeer = (ledger: Ledger, peerId: NodeId) => {
  // TODO: void all pending transfers?
  assert(ledger)
  assert(peerId)
}
