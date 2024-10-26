import { getDesiredSendAmount } from "../stream/send-money"
import type { ConnectionState } from "./state"

/**
 * Check if any streams have work to do.
 *
 * If `includePending` is true, this function will also look for pending work
 * that is already being worked on but may fail and have to be retried.
 *
 * @param state - The state of the connection.
 * @param includePending - Whether to consider pending work that is already being worked on.
 * @returns Whether there is work to do.
 */
export function hasWorkToDo(state: ConnectionState, includePending = false) {
  if (state.isClosed) return false

  for (const stream of state.streams.values()) {
    if (stream.isClosed && !stream.isRemoteClosed) continue

    const remainingSendAmount =
      getDesiredSendAmount(stream) +
      (includePending ? stream.sendHoldAmount : 0n)

    if (remainingSendAmount > state.context.policy.deMinimisAmount) return true
  }

  return false
}
