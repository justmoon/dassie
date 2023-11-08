import { castImmutable, enableMapSet, produce } from "immer"

import { createStore } from "@dassie/lib-reactive"

enableMapSet()

export const BtpTokensStore = () =>
  createStore(castImmutable(new Set<string>()), {
    addToken: (token: string) =>
      produce((draft) => {
        draft.add(token)
      }),
    removeToken: (token: string) =>
      produce((draft) => {
        draft.delete(token)
      }),
  })
