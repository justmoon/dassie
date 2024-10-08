import { column, table } from "@dassie/lib-sqlite"

export const registrationsTable = table({
  name: "registrations",
  columns: {
    node: column().primaryKey().notNull().type("INTEGER"),
    registered_at: column().notNull().type("INTEGER"),
    renewed_at: column().notNull().type("INTEGER"),
  },
})
