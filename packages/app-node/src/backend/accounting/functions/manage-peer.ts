import assert from "node:assert"

import { isFailure } from "@dassie/lib-type-utils"

import { NodeId } from "../../peer-protocol/types/node-id"
import { SettlementSchemeId } from "../../peer-protocol/types/settlement-scheme-id"
import { Ledger } from "../stores/ledger"

const DEFAULT_CREDIT = 1_100_000_000n

export const initializePeer = (
  ledger: Ledger,
  settlementSchemeId: SettlementSchemeId,
  peerId: NodeId,
) => {
  ledger.createAccount(`${settlementSchemeId}/peer/${peerId}/interledger`, {
    limit: "debits_must_not_exceed_credits",
  })
  ledger.createAccount(`${settlementSchemeId}/peer/${peerId}/settlement`)
  ledger.createAccount(`${settlementSchemeId}/peer/${peerId}/trust`, {
    limit: "credits_must_not_exceed_debits",
  })

  // Extend initial trust limit
  const result = ledger.createTransfer({
    debitAccountPath: `${settlementSchemeId}/peer/${peerId}/trust`,
    creditAccountPath: `${settlementSchemeId}/peer/${peerId}/interledger`,
    amount: DEFAULT_CREDIT,
  })

  if (isFailure(result)) {
    throw new Error(`Could not extend initial trust to peer: ${result.name}`)
  }
}

export const cleanupPeer = (ledger: Ledger, peerId: NodeId) => {
  // TODO: void all pending transfers?
  assert(ledger)
  assert(peerId)
}
