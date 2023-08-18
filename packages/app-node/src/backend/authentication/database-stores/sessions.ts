import { enableMapSet, produce } from "immer"

import { Reactor, createStore } from "@dassie/lib-reactive"

import { databasePlain } from "../../database/open-database"
import { SessionToken } from "../types/session-token"

enableMapSet()

export const sessionsStore = (reactor: Reactor) => {
  const database = reactor.use(databasePlain)

  const sessionSet = new Set(
    database.tables.sessions.selectAll().map(({ token }) => token)
  )

  return createStore(sessionSet, {
    addSession: (sessionToken: SessionToken) =>
      produce((draft) => {
        draft.add(sessionToken)
        database.tables.sessions.insertOne({
          token: sessionToken,
        })
      }),
  })
}
