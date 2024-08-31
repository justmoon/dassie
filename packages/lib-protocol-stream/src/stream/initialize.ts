import { createTopic } from "@dassie/lib-reactive"

import type { StreamState } from "./state"

export function createInitialStreamState(): StreamState {
  return {
    sendMaximum: 0n,
    sendHoldAmount: 0n,
    sentAmount: 0n,

    receiveMaximum: 0n,
    receivedAmount: 0n,

    topics: {
      money: createTopic(),
    },
  }
}
