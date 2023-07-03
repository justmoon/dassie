import assert from "node:assert"

import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"
import { UnreachableCaseError, isFailure } from "@dassie/lib-type-utils"

import {
  processSettlementPrepare,
  processSettlementResult,
} from "../accounting/functions/process-settlement"
import { Ledger, ledgerStore } from "../accounting/stores/ledger"
import { nodeTableStore } from "../peer-protocol/stores/node-table"
import { NodeId } from "../peer-protocol/types/node-id"
import { SubnetId } from "../peer-protocol/types/subnet-id"
import { manageSubnetInstances } from "../subnets/manage-subnet-instances"

const logger = createLogger("das:node:send-outgoing-settlements")

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

const calculateSettlementAmount = (
  ledger: Ledger,
  subnetId: SubnetId,
  peerId: NodeId
) => {
  const peerInterledgerAccount = ledger.getAccount(
    `${subnetId}/peer/${peerId}/interledger`
  )
  const peerTrustAccount = ledger.getAccount(`${subnetId}/peer/${peerId}/trust`)
  const peerSettlementAccount = ledger.getAccount(
    `${subnetId}/peer/${peerId}/settlement`
  )

  assert(peerInterledgerAccount, "peer interledger account not found")
  assert(peerTrustAccount, "peer trust account not found")
  assert(peerSettlementAccount, "peer settlement account not found")

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
  createActor((sig, peerId: NodeId) => {
    const ledger = sig.use(ledgerStore)
    const nodeTable = sig.use(nodeTableStore)
    const subnetManager = sig.use(manageSubnetInstances)

    sig.interval(() => {
      const peerState = nodeTable.read().get(peerId)?.peerState

      assert(peerState?.id === "peered", "peer state must be 'peered'")

      const { subnetId } = peerState

      const subnetActor = subnetManager.get(subnetId)

      if (!subnetActor) {
        logger.warn("subnet actor not found, skipping settlement", { subnetId })
        return
      }

      const settlementAmount = calculateSettlementAmount(
        ledger,
        subnetId,
        peerId
      )

      if (settlementAmount > 0n) {
        const settlementTransfer = processSettlementPrepare(
          ledger,
          subnetId,
          peerId,
          settlementAmount,
          "outgoing"
        )

        if (isFailure(settlementTransfer)) {
          switch (settlementTransfer.name) {
            case "InvalidAccountFailure": {
              throw new Error(
                `Settlement failed, invalid ${settlementTransfer.whichAccount} account ${settlementTransfer.accountPath}`
              )
            }

            case "ExceedsCreditsFailure": {
              throw new Error(`Settlement failed, exceeds credits`)
            }

            case "ExceedsDebitsFailure": {
              throw new Error(`Settlement failed, exceeds debits`)
            }

            default: {
              throw new UnreachableCaseError(settlementTransfer)
            }
          }
        }

        subnetActor
          .ask("settle", {
            amount: settlementAmount,
            peerId,
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
