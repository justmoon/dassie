import { assert } from "@dassie/lib-logger"
import { createActor, createMapped } from "@dassie/lib-reactive"
import { UnreachableCaseError, isFailure, tell } from "@dassie/lib-type-utils"

import type { LedgerId } from "../accounting/constants/ledgers"
import { processSettlementPrepare } from "../accounting/functions/process-settlement"
import { type Ledger, LedgerStore } from "../accounting/stores/ledger"
import type {
  DassieActorContext,
  DassieReactor,
} from "../base/types/dassie-base"
import { settlement as logger } from "../logger/instances"
import { PeersSignal } from "../peer-protocol/computed/peers"
import { SendPeerMessage } from "../peer-protocol/functions/send-peer-message"
import { NodeTableStore } from "../peer-protocol/stores/node-table"
import type { NodeId } from "../peer-protocol/types/node-id"
import { GetLedgerIdForSettlementScheme } from "./functions/get-ledger-id"
import { ManageSettlementSchemeInstancesActor } from "./manage-settlement-scheme-instances"
import { PendingSettlementsMap } from "./values/pending-settlements-map"

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
  ledgerId: LedgerId,
  peerId: NodeId,
) => {
  const peerInterledgerAccount = ledger.getAccount(
    `${ledgerId}:assets/interledger/${peerId}`,
  )
  const peerTrustAccount = ledger.getAccount(
    `${ledgerId}:contra/trust/${peerId}`,
  )
  const assetsOnLedgerAccount = ledger.getAccount(
    `${ledgerId}:assets/settlement`,
  )

  assert(logger, !!peerInterledgerAccount, "peer interledger account not found")
  assert(logger, !!peerTrustAccount, "peer trust account not found")
  assert(logger, !!assetsOnLedgerAccount, "on ledger assets account not found")

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
    1 + SETTLEMENT_RATIO / 2,
  )

  if (balance < settlementThreshold) {
    return 0n
  }

  let proposedSettlementAmount = balance - settlementMidpoint

  if (proposedSettlementAmount < 0) proposedSettlementAmount = 0n
  if (proposedSettlementAmount > balance) proposedSettlementAmount = balance

  if (proposedSettlementAmount > 0n) {
    logger.debug?.("proposing settlement", {
      balance,
      outgoingCredit,
      incomingCredit,
      settlementMidpoint,
      settlementThreshold,
      proposedSettlementAmount,
    })
  }

  return proposedSettlementAmount
}

export const SendOutgoingSettlementsActor = (reactor: DassieReactor) => {
  const ledger = reactor.use(LedgerStore)
  const nodeTable = reactor.use(NodeTableStore)
  const settlementSchemeManager = reactor.use(
    ManageSettlementSchemeInstancesActor,
  )
  const pendingSettlementsMap = reactor.use(PendingSettlementsMap)
  const sendPeerMessage = reactor.use(SendPeerMessage)
  const getLedgerIdForSettlementScheme = reactor.use(
    GetLedgerIdForSettlementScheme,
  )

  return createMapped(reactor, PeersSignal, (peerId) =>
    createActor((sig: DassieActorContext) => {
      // eslint-disable-next-line unicorn/consistent-function-scoping
      function sendOutgoingSettlements() {
        const peerState = nodeTable.read().get(peerId)?.peerState

        assert(
          logger,
          peerState?.id === "peered",
          "peer state must be 'peered'",
        )

        const { settlementSchemeId, settlementSchemeState } = peerState

        const settlementSchemeActor =
          settlementSchemeManager.get(settlementSchemeId)

        if (!settlementSchemeActor) {
          logger.warn(
            "settlement scheme actor not found, skipping settlement",
            {
              settlementSchemeId,
            },
          )
          return
        }

        const ledgerId = getLedgerIdForSettlementScheme(settlementSchemeId)

        const settlementAmount = calculateSettlementAmount(
          ledger,
          ledgerId,
          peerId,
        )

        if (settlementAmount > 0n) {
          settlementSchemeActor.api.prepareSettlement
            .ask({
              amount: settlementAmount,
              peerId,
              peerState: settlementSchemeState,
            })
            .then(async ({ message, settlementId, execute }) => {
              const settlementTransfer = processSettlementPrepare(
                ledger,
                ledgerId,
                peerId,
                settlementAmount,
                "outgoing",
              )

              if (isFailure(settlementTransfer)) {
                switch (settlementTransfer.name) {
                  case "InvalidAccountFailure": {
                    throw new Error(
                      `Settlement failed, invalid ${settlementTransfer.whichAccount} account ${settlementTransfer.accountPath}`,
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

              const settlementKey = `${settlementSchemeId}:${settlementId}`
              pendingSettlementsMap.set(settlementKey, settlementTransfer)

              tell(() =>
                sendPeerMessage({
                  destination: peerId,
                  message: {
                    type: "settlement",
                    value: {
                      settlementSchemeId,
                      amount: settlementAmount,
                      settlementSchemeData: message,
                    },
                  },
                }),
              )

              try {
                await execute()
              } catch (error: unknown) {
                logger.warn("failed to execute outbound settlement", { error })
              }
            })
            .catch((error: unknown) => {
              logger.warn("failed to prepare outbound settlement", { error })
            })
        }
      }

      sig
        .task({
          handler: sendOutgoingSettlements,
          interval: SETTLEMENT_CHECK_INTERVAL,
        })
        .schedule()
    }),
  )
}
