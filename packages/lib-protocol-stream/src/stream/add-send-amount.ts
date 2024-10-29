import { isFailure } from "@dassie/lib-type-utils"

import { assertConnectionCanSendMoney } from "../connection/assert-can-send"
import type { ConnectionState } from "../connection/state"
import type { StreamState } from "./state"

interface AddSendAmountOptions {
  connectionState: ConnectionState
  state: StreamState
  amount: bigint
}

export function addSendAmount({
  connectionState,
  state,
  amount,
}: AddSendAmountOptions) {
  {
    const result = assertConnectionCanSendMoney(connectionState)
    if (isFailure(result)) return result
  }

  state.sendMaximum += amount

  return
}
