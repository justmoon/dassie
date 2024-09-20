import { column, table } from "@dassie/lib-sqlite"

import type { SessionToken } from "../types/session-token"

export const sessionsTable = table({
  name: "sessions",
  columns: {
    token: column().type("TEXT").typescriptType<SessionToken>().primaryKey(),
  },
})
