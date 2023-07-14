import { Reactor, createStore } from "@dassie/lib-reactive"

import { databasePlain } from "../../database/open-database"

export const settlementSchemesStore = (reactor: Reactor) => {
  const database = reactor.use(databasePlain)

  const settlementSchemeRows = database.tables.settlementSchemes.selectAll()

  return createStore(settlementSchemeRows, {})
}
