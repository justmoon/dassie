import type { InferTopics } from "../types/infer-topics"

export type StreamEvents = {
  money: bigint
  moneySent: bigint
  closed: void
}

export interface StreamState {
  sendMaximum: bigint
  sendHoldAmount: bigint
  sentAmount: bigint

  receiveMaximum: bigint
  receivedAmount: bigint

  topics: InferTopics<StreamEvents>

  isClosed: boolean
  isRemoteClosed: boolean
}
