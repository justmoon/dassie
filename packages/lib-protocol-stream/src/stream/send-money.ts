import type { StreamState } from "./state"

export function getDesiredSendAmount(streamState: StreamState) {
  return (
    streamState.sendMaximum -
    streamState.sendHoldAmount -
    streamState.sentAmount
  )
}

export function prepareSend(streamState: StreamState, amount: bigint) {
  if (
    amount >
    streamState.sendMaximum -
      streamState.sendHoldAmount -
      streamState.sentAmount
  ) {
    throw new Error("Cannot hold more than the maximum amount")
  }

  streamState.sendHoldAmount += amount
}

export function fulfillSend(streamState: StreamState, amount: bigint) {
  if (amount > streamState.sendHoldAmount) {
    throw new Error("Cannot fulfill more than the amount on hold")
  }

  streamState.sentAmount += amount
  streamState.sendHoldAmount -= amount

  streamState.topics.moneySent.emit(amount)
}

export function rejectSend(streamState: StreamState, amount: bigint) {
  if (amount > streamState.sendHoldAmount) {
    throw new Error("Cannot reject more than the amount on hold")
  }

  streamState.sendHoldAmount -= amount
}
