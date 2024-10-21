import { UINT64_MAX } from "@dassie/lib-oer"
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
      moneySent: createTopic(),
      closed: createTopic(),
    },

    isClosed: false,

    remoteReceivedAmount: 0n,
    remoteReceiveMaximum: UINT64_MAX,
    isRemoteClosed: false,
  }
}
