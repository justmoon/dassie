import { produce } from "immer"

import { Reactor, createStore } from "@dassie/lib-reactive"

import { databasePlain } from "../../database/open-database"
import { SessionToken } from "../types/session-token"

export const sessionsStore = (reactor: Reactor) => {
  const database = reactor.use(databasePlain)

  const sessionRows = database.tables.sessions.selectAll()

  return createStore(sessionRows, {
    addSession: (sessionToken: SessionToken) =>
      produce((draft) => {
        draft.push({
          token: sessionToken,
        })
        database.tables.sessions.insertOne({
          token: sessionToken,
        })
      }),
  })
}
