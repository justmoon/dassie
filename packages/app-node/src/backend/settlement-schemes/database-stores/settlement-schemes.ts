import { produce } from "immer"

import { Reactor, createStore } from "@dassie/lib-reactive"

import { databasePlain } from "../../database/open-database"
import { SettlementSchemeId } from "../../peer-protocol/types/settlement-scheme-id"

export const settlementSchemesStore = (reactor: Reactor) => {
  const database = reactor.use(databasePlain)

  const settlementSchemeRows = database.tables.settlementSchemes.selectAll()

  return createStore(settlementSchemeRows, {
    addSettlementScheme: (
      settlementSchemeId: SettlementSchemeId,
      config: object
    ) =>
      produce((draft) => {
        draft.push({
          id: settlementSchemeId,
          config,
        })
        database.tables.settlementSchemes.insertOne({
          id: settlementSchemeId,
          config,
        })
      }),
  })
}
