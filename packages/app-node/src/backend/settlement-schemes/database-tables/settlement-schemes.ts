import { column, table } from "@dassie/lib-sqlite"

import { SettlementSchemeId } from "../../peer-protocol/types/settlement-scheme-id"

export const settlementSchemesTable = table({
  name: "settlement_schemes",
  columns: {
    id: column()
      .type("TEXT")
      .primaryKey()
      .serialize((value: SettlementSchemeId) => value)
      .deserialize((value) => value as SettlementSchemeId),
    config: column()
      .type("TEXT")
      .required()
      .serialize((value: object) => JSON.stringify(value))
      .deserialize((value) => JSON.parse(value) as object),
  },
})
