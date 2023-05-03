import { createActor } from "@dassie/lib-reactive"

import {
  type PeerBalanceState,
  peerBalanceMapStore,
} from "../balances/stores/peer-balance-map"
import type { PerPeerParameters } from "../peer-protocol/run-per-peer-effects"

const SETTLEMENT_CHECK_INTERVAL = 10_000
const SETTLEMENT_RATIO = 0.8

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

const calculateSettlementAmount = (balanceInfo: PeerBalanceState) => {
  const { balance, incomingCredit, outgoingCredit, incomingPending } =
    balanceInfo

  const settlementMidpoint =
    (incomingCredit + outgoingCredit) / 2n - incomingCredit
  const settlementThreshold =
    multiplyAmountWithRatio(
      settlementMidpoint + incomingCredit,
      SETTLEMENT_RATIO
    ) - incomingCredit

  if (balance > settlementThreshold) {
    return 0n
  }

  const proposedSettlementAmount = settlementMidpoint - balance
  const maxSettlementAmount = outgoingCredit - balance - incomingPending

  return proposedSettlementAmount < 0
    ? 0n
    : proposedSettlementAmount > maxSettlementAmount
    ? maxSettlementAmount
    : proposedSettlementAmount
}

export const sendOutgoingSettlements = () =>
  createActor((sig, { peerKey, subnetActor }: PerPeerParameters) => {
    const subnetActorInstance = sig.use(subnetActor)
    const balanceMapStore = sig.use(peerBalanceMapStore)

    sig.interval(() => {
      const balanceInfo = balanceMapStore.read().get(peerKey)

      if (balanceInfo == undefined) {
        throw new Error(`Unable to read peer balance for ${peerKey}`)
      }

      const settlementAmount = calculateSettlementAmount(balanceInfo)

      if (settlementAmount > 0n) {
        balanceMapStore.handleOutgoingSettlementPrepared(
          peerKey,
          settlementAmount
        )

        subnetActorInstance
          .ask("settle", {
            amount: settlementAmount,
            peerKey,
          })
          .then(() => {
            balanceMapStore.handleOutgoingSettlementFulfilled(
              peerKey,
              settlementAmount
            )
          })
          .catch(() => {
            balanceMapStore.handleOutgoingSettlementRejected(
              peerKey,
              settlementAmount
            )
          })
      }
    }, SETTLEMENT_CHECK_INTERVAL)
  })
