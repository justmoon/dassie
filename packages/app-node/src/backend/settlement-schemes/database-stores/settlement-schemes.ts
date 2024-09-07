import { produce } from "immer"

import { type Reactor, createStore } from "@dassie/lib-reactive"

import { Database } from "../../database/open-database"
import type { SettlementSchemeId } from "../../peer-protocol/types/settlement-scheme-id"

export const SettlementSchemesStore = (reactor: Reactor) => {
  const database = reactor.use(Database)

  const settlementSchemeRows = database.tables.settlementSchemes
    .selectAll()
    .map(({ id, config }) => ({
      id,
      config: JSON.parse(config) as object,
    }))

  return createStore(settlementSchemeRows).actions({
    addSettlementScheme: (
      settlementSchemeId: SettlementSchemeId,
      config: object,
    ) =>
      produce((draft) => {
        draft.push({
          id: settlementSchemeId,
          config,
        })
        database.tables.settlementSchemes.insertOne({
          id: settlementSchemeId,
          config: JSON.stringify(config),
        })
      }),
  })
}
