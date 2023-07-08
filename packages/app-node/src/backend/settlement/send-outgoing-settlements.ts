import assert from "node:assert"

import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"
import { UnreachableCaseError, isFailure } from "@dassie/lib-type-utils"

import { processSettlementPrepare } from "../accounting/functions/process-settlement"
import { Ledger, ledgerStore } from "../accounting/stores/ledger"
import { sendPeerMessage } from "../peer-protocol/actors/send-peer-message"
import { nodeTableStore } from "../peer-protocol/stores/node-table"
import { NodeId } from "../peer-protocol/types/node-id"
import { SettlementSchemeId } from "../peer-protocol/types/settlement-scheme-id"
import { manageSettlementSchemeInstances } from "../settlement-schemes/manage-settlement-scheme-instances"

const logger = createLogger("das:node:send-outgoing-settlements")

const SETTLEMENT_CHECK_INTERVAL = 4000
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
  settlementSchemeId: SettlementSchemeId,
  peerId: NodeId
) => {
  const peerInterledgerAccount = ledger.getAccount(
    `${settlementSchemeId}/peer/${peerId}/interledger`
  )
  const peerTrustAccount = ledger.getAccount(
    `${settlementSchemeId}/peer/${peerId}/trust`
  )
  const peerSettlementAccount = ledger.getAccount(
    `${settlementSchemeId}/peer/${peerId}/settlement`
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
    const settlementSchemeManager = sig.use(manageSettlementSchemeInstances)

    sig.interval(() => {
      const peerState = nodeTable.read().get(peerId)?.peerState

      assert(peerState?.id === "peered", "peer state must be 'peered'")

      const { settlementSchemeId: settlementSchemeId } = peerState

      const settlementSchemeActor =
        settlementSchemeManager.get(settlementSchemeId)

      if (!settlementSchemeActor) {
        logger.warn("settlement scheme actor not found, skipping settlement", {
          settlementSchemeId,
        })
        return
      }

      const settlementAmount = calculateSettlementAmount(
        ledger,
        settlementSchemeId,
        peerId
      )

      if (settlementAmount > 0n) {
        const settlementTransfer = processSettlementPrepare(
          ledger,
          settlementSchemeId,
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

        settlementSchemeActor
          .ask("settle", {
            amount: settlementAmount,
            peerId,
          })
          .then((data) => {
            return data
          })
          .then(({ proof }) => {
            ledger.postPendingTransfer(settlementTransfer)

            sig.use(sendPeerMessage).tell("send", {
              destination: peerId,
              message: {
                type: "settlement",
                value: {
                  settlementSchemeId,
                  amount: settlementAmount,
                  proof,
                },
              },
            })
          })
          .catch(() => {
            ledger.voidPendingTransfer(settlementTransfer)
          })
      }
    }, SETTLEMENT_CHECK_INTERVAL)
  })
