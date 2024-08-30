import type { StreamState } from "./state"

export function createInitialStreamState(): StreamState {
  return {
    sendMaximum: 0n,
    sendHoldAmount: 0n,
    sentAmount: 0n,
  }
}
