import type { Topic } from "@dassie/lib-reactive"

export interface StreamState {
  sendMaximum: bigint
  sendHoldAmount: bigint
  sentAmount: bigint

  receiveMaximum: bigint
  receivedAmount: bigint

  topics: {
    money: Topic<bigint>
  }
}
