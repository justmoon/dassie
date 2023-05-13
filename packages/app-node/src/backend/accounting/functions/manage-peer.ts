import assert from "node:assert"

import { NodeTableKey } from "../../peer-protocol/stores/node-table"
import { Ledger } from "../stores/ledger"

const DEFAULT_CREDIT = 1_000_000n

export const initializePeer = (ledger: Ledger, peerKey: NodeTableKey) => {
  ledger.createAccount(`peer/${peerKey}/interledger`, {
    limit: "debits_must_not_exceed_credits",
  })
  ledger.createAccount(`peer/${peerKey}/settlement`)
  ledger.createAccount(`peer/${peerKey}/trust`, {
    limit: "credits_must_not_exceed_debits",
  })

  // Extend initial trust limit
  ledger.createTransfer({
    key: `${peerKey}/initialTrust`,
    debitAccountPath: `peer/${peerKey}/trust`,
    creditAccountPath: `peer/${peerKey}/interledger`,
    amount: DEFAULT_CREDIT,
  })
}

export const cleanupPeer = (_ledger: Ledger, _peerKey: NodeTableKey) => {
  // TODO: void all pending transfers?
  assert(_ledger)
  assert(_peerKey)
}
