import { getDesiredSendAmount } from "../stream/send-money"
import type { ConnectionState } from "./state"

export function hasWorkToDo(state: ConnectionState) {
  for (const stream of state.streams.values()) {
    const remainingSendAmount = getDesiredSendAmount(stream)
    if (remainingSendAmount > 0n) return true
  }

  return false
}
