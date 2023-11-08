import { castImmutable, enableMapSet, produce } from "immer"

import { Reactor, createStore } from "@dassie/lib-reactive"

import { Database } from "../../database/open-database"
import { BtpToken } from "../types/btp-token"

enableMapSet()

export const BtpTokensStore = (reactor: Reactor) => {
  const database = reactor.use(Database)

  const btpTokenSet = new Set(
    database.tables.btpTokens.selectAll().map(({ token }) => token),
  )

  return createStore(castImmutable(btpTokenSet), {
    addToken: (token: BtpToken) =>
      produce((draft) => {
        draft.add(token)
        database.tables.btpTokens.insertOne({ token })
      }),
    removeToken: (token: BtpToken) =>
      produce((draft) => {
        draft.delete(token)
        database.tables.btpTokens.delete({ token })
      }),
  })
}
