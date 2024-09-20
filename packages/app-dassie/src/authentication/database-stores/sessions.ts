import { castImmutable, enableMapSet, produce } from "immer"

import { type Reactor, createStore } from "@dassie/lib-reactive"

import { Database } from "../../database/open-database"
import type { SessionToken } from "../types/session-token"

enableMapSet()

export const SessionsStore = (reactor: Reactor) => {
  const database = reactor.use(Database)

  const sessionSet = new Set(
    database.tables.sessions.selectAll().map(({ token }) => token),
  )

  return createStore(castImmutable(sessionSet)).actions({
    addSession: (token: SessionToken) =>
      produce((draft) => {
        draft.add(token)
        database.tables.sessions.insertOne({ token })
      }),
    removeSession: (token: SessionToken) =>
      produce((draft) => {
        draft.delete(token)
        database.tables.sessions.delete({ token })
      }),
  })
}
