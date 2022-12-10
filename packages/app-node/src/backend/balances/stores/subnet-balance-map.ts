import produce, { enableMapSet } from "immer"

import { createStore } from "@dassie/lib-reactive"

enableMapSet()

export const subnetBalanceMapStore = () =>
  createStore(new Map<string, bigint>(), {
    setBalance: (subnetId: string, balance: bigint) =>
      produce((draft) => {
        draft.set(subnetId, balance)
      }),
    clearBalance: (subnetId: string) =>
      produce((draft) => {
        draft.delete(subnetId)
      }),
  })
