import { column, table } from "@dassie/lib-sqlite"

import { SessionToken } from "../types/session-token"

export const sessionsTable = table({
  name: "sessions",
  columns: {
    token: column().type("TEXT").typescriptType<SessionToken>().primaryKey(),
  },
})
