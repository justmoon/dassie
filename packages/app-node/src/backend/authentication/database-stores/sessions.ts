import { enableMapSet, produce } from "immer"

import { Reactor, createStore } from "@dassie/lib-reactive"

import { Database } from "../../database/open-database"
import { SessionToken } from "../types/session-token"

enableMapSet()

export const SessionsStore = (reactor: Reactor) => {
  const database = reactor.use(Database)

  const sessionSet = new Set(
    database.tables.sessions.selectAll().map(({ token }) => token),
  )

  return createStore(sessionSet, {
    addSession: (sessionToken: SessionToken) =>
      produce((draft) => {
        draft.add(sessionToken)
        database.tables.sessions.insertOne({
          token: sessionToken,
        })
      }),
    removeSession: (sessionToken: SessionToken) =>
      produce((draft) => {
        draft.delete(sessionToken)
        database.tables.sessions
          .select()
          .where({ equals: { token: sessionToken } })
          .delete()
      }),
  })
}
