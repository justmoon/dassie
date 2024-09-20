import { column, table } from "@dassie/lib-sqlite"

export const acmeTokensTable = table({
  name: "acme_tokens",
  columns: {
    token: column().type("TEXT").primaryKey(),
    key_authorization: column().type("TEXT").notNull(),
    expires: column().type("TEXT"),
  },
})
