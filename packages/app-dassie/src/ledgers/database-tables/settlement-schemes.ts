import { column, table } from "@dassie/lib-sqlite"

import type { SettlementSchemeId } from "../../peer-protocol/types/settlement-scheme-id"

export const settlementSchemesTable = table({
  name: "settlement_schemes",
  columns: {
    id: column().type("TEXT").typescriptType<SettlementSchemeId>().primaryKey(),
    config: column().type("TEXT").notNull(),
  },
})
