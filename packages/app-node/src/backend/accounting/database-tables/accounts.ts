import { column, table } from "@dassie/lib-sqlite"

import type { AccountPath } from "../types/account-paths"

export const accountsTable = table({
  name: "accounts",
  columns: {
    path: column().type("TEXT").typescriptType<AccountPath>().primaryKey(),
    debits_posted: column().type("INTEGER").notNull().default(0n),
    credits_posted: column().type("INTEGER").notNull().default(0n),
  },
})
