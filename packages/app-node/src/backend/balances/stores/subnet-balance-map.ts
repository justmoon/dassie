import produce, { enableMapSet } from "immer"

import { createStore } from "@dassie/lib-reactive"

enableMapSet()

export const subnetBalanceMapStore = () =>
  createStore(new Map<string, bigint>(), {
    setBalance: (subnetId: string, balance: bigint) =>
      produce((draft) => {
        draft.set(subnetId, balance)
      }),
    adjustBalance: (subnetId: string, adjustment: bigint) =>
      produce((draft) => {
        const currentBalance = draft.get(subnetId) ?? 0n
        draft.set(subnetId, currentBalance + adjustment)
      }),
    clearBalance: (subnetId: string) =>
      produce((draft) => {
        draft.delete(subnetId)
      }),
  })
