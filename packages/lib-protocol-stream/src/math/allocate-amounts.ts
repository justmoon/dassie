import {
  AMOUNT_EXCEEDS_MAXIMUM_RECEIVE_AMOUNT,
  type AmountExceedsMaximumReceiveAmountFailure,
} from "./failures/amount-exceeds-maximum-receive-amount"
import { min } from "./min-max"
import { type Ratio, multiplyByRatio } from "./ratio"

export interface StreamReceiveListEntry {
  readonly streamId: number
  readonly shares: bigint
  readonly receivedAmount: bigint
  readonly receiveMaximum: bigint
}

export interface StreamAllocationListEntry {
  readonly streamId: number
  amount: bigint
}

export function allocateAmounts(
  totalShares: bigint,
  totalAmount: bigint,
  streamReceiveList: readonly StreamReceiveListEntry[],
): StreamAllocationListEntry[] | AmountExceedsMaximumReceiveAmountFailure {
  if (streamReceiveList.length === 0) return []

  let remainingAmount = totalAmount
  let shareRatio: Ratio = [remainingAmount, totalShares]

  const allocationList: StreamAllocationListEntry[] = streamReceiveList.map(
    ({ streamId }) => ({
      streamId,
      amount: 0n,
    }),
  )

  for (;;) {
    const lastRemainingAmount = remainingAmount

    totalShares = 0n
    for (const [index, streamReceiveEntry] of streamReceiveList.entries()) {
      const amountByShare = multiplyByRatio(
        streamReceiveEntry.shares,
        shareRatio,
      )

      const canReceive =
        streamReceiveEntry.receiveMaximum -
        streamReceiveEntry.receivedAmount -
        allocationList[index]!.amount

      const allocatedAmount = min(
        min(amountByShare, canReceive),
        remainingAmount,
      )

      remainingAmount -= allocatedAmount

      // If this stream can receive more, we'll throw our shares back in the ring
      if (allocatedAmount < canReceive) {
        totalShares += streamReceiveEntry.shares
      }

      allocationList[index]!.amount += allocatedAmount
    }

    if (remainingAmount === 0n) {
      return allocationList
    }

    if (totalShares === 0n) {
      return AMOUNT_EXCEEDS_MAXIMUM_RECEIVE_AMOUNT
    }

    // If we did not allocate any funds during the last round, it means that
    // we are down to the last few amount units and things are getting rounded
    // down to zero. We artificially increate the share ratio to ensure that the
    // last few units are allocated somewhere.
    shareRatio =
      remainingAmount === lastRemainingAmount ?
        [remainingAmount, 1n]
      : [remainingAmount, totalShares]
  }
}
