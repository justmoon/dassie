import assert from "node:assert"

import { createActor } from "@dassie/lib-reactive"
import { isFailure } from "@dassie/lib-type-utils"

import {
  processSettlementPrepare,
  processSettlementResult,
} from "../accounting/functions/process-settlement"
import { Ledger, ledgerStore } from "../accounting/stores/ledger"
import type { PerPeerParameters } from "../peer-protocol/run-per-peer-actors"
import { NodeTableKey } from "../peer-protocol/stores/node-table"

const SETTLEMENT_CHECK_INTERVAL = 10_000
const SETTLEMENT_RATIO = 0.2

/**
 * The precision to use when calculating ratios.
 */
const RATIO_PRECISION = 8

/**
 *
 * @param amount - The amount to multiply
 * @param ratio - The ratio to multiply by
 * @returns
 */
const multiplyAmountWithRatio = (amount: bigint, ratio: number) => {
  const ratioBigInt = BigInt(Math.round(ratio * 10 ** RATIO_PRECISION))

  return (amount * ratioBigInt) / BigInt(10 ** RATIO_PRECISION)
}

const calculateSettlementAmount = (ledger: Ledger, peerKey: NodeTableKey) => {
  const peerInterledgerAccount = ledger.getAccount(
    `peer/${peerKey}/interledger`
  )
  const peerTrustAccount = ledger.getAccount(`peer/${peerKey}/trust`)
  const peerSettlementAccount = ledger.getAccount(`peer/${peerKey}/settlement`)

  assert(peerInterledgerAccount)
  assert(peerTrustAccount)
  assert(peerSettlementAccount)

  const balance =
    peerInterledgerAccount.creditsPosted -
    peerInterledgerAccount.debitsPosted -
    peerInterledgerAccount.debitsPending

  const outgoingCredit =
    peerTrustAccount.debitsPosted - peerTrustAccount.creditsPosted

  // TODO: Get actual peer credit to use here, for now we assume they extended us the same credit that we extended them
  const incomingCredit = outgoingCredit

  const settlementMidpoint = (incomingCredit + outgoingCredit) / 2n
  const settlementThreshold = multiplyAmountWithRatio(
    settlementMidpoint,
    1 + SETTLEMENT_RATIO / 2
  )

  if (balance < settlementThreshold) {
    return 0n
  }

  const proposedSettlementAmount = balance - settlementMidpoint

  return proposedSettlementAmount < 0
    ? 0n
    : proposedSettlementAmount > balance
    ? balance
    : proposedSettlementAmount
}

export const sendOutgoingSettlements = () =>
  createActor((sig, { peerKey, subnetActor }: PerPeerParameters) => {
    const subnetActorInstance = sig.use(subnetActor)
    const ledger = sig.use(ledgerStore)

    sig.interval(() => {
      const settlementAmount = calculateSettlementAmount(ledger, peerKey)

      if (settlementAmount > 0n) {
        const settlementTransfer = processSettlementPrepare(
          ledger,
          peerKey,
          settlementAmount,
          "outgoing"
        )

        if (isFailure(settlementTransfer)) {
          throw new Error(
            `Settlement failed, invalid ${settlementTransfer.whichAccount} account ${settlementTransfer.accountPath}`
          )
        }

        subnetActorInstance
          .ask("settle", {
            amount: settlementAmount,
            peerKey,
          })
          .then(() => {
            processSettlementResult(ledger, settlementTransfer, "fulfill")
          })
          .catch(() => {
            processSettlementResult(ledger, settlementTransfer, "reject")
          })
      }
    }, SETTLEMENT_CHECK_INTERVAL)
  })
