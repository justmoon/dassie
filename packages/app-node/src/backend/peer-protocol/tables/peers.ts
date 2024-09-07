import { column, table } from "@dassie/lib-sqlite"

import type { SettlementSchemeId } from "../types/settlement-scheme-id"

export const peersTable = table({
  name: "peers",
  columns: {
    node: column().primaryKey().notNull().type("INTEGER"),
    settlement_scheme_id: column()
      .type("TEXT")
      .typescriptType<SettlementSchemeId>()
      .notNull(),
    settlement_scheme_state: column().type("TEXT").notNull(),
  },
})
