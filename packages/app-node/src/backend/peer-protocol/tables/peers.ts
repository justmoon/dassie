import { column, table } from "@dassie/lib-sqlite"

import { SettlementSchemeId } from "../types/settlement-scheme-id"

export const peersTable = table({
  name: "peers",
  columns: {
    node: column().primaryKey().notNull().type("INTEGER"),
    settlement_scheme_id: column()
      .type("TEXT")
      .notNull()
      .serialize((value: SettlementSchemeId) => value)
      .deserialize((value) => value as SettlementSchemeId),
  },
})
