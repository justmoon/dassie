import { isFailure } from "@dassie/lib-type-utils"

import type { NodeId } from "../../peer-protocol/types/node-id"
import type { LedgerId } from "../constants/ledgers"
import type { Ledger } from "../stores/ledger"
import type {
  AssetsInterledgerPeerAccount,
  ContraTrustPeerAccount,
} from "../types/account-paths"

const DEFAULT_CREDIT = 1_100_000_000n

export const initializePeer = (
  ledger: Ledger,
  ledgerId: LedgerId,
  peerId: NodeId,
) => {
  const interledgerPath: AssetsInterledgerPeerAccount = `${ledgerId}:assets/interledger/${peerId}`
  const trustPath: ContraTrustPeerAccount = `${ledgerId}:contra/trust/${peerId}`

  ledger.createAccount(interledgerPath, {
    limit: "debits_must_not_exceed_credits",
  })
  ledger.createAccount(trustPath, {
    limit: "credits_must_not_exceed_debits",
  })

  // Extend initial trust limit
  const result = ledger.createTransfer({
    debitAccountPath: trustPath,
    creditAccountPath: interledgerPath,
    amount: DEFAULT_CREDIT,
  })

  if (isFailure(result)) {
    throw new Error(`Could not extend initial trust to peer: ${result.name}`)
  }
}

export const cleanupPeer = (_ledger: Ledger, _peerId: NodeId) => {
  // TODO: void all pending transfers?
}
