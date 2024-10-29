import type { InferTopics } from "../types/infer-topics"

export interface RemoteMoneyEvent {
  readonly receivedAmount: bigint
  readonly receiveMaximum: bigint
}

export type StreamEvents = {
  money: bigint
  moneySent: bigint
  remoteMoney: RemoteMoneyEvent
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

  remoteReceivedAmount: bigint
  remoteReceiveMaximum: bigint
  isRemoteClosed: boolean
}
