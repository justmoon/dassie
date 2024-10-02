import type { MigrationDefinition } from "@dassie/lib-sqlite"

export const CREATE_R1_SCALARS_TABLE = `
CREATE TABLE scalar (
  -- String key used to index the value
  key TEXT PRIMARY KEY,

  -- Value (type is managed by the application)
  value ANY NOT NULL
) STRICT, WITHOUT ROWID
`

const migration: MigrationDefinition = {
  version: 1,
  up: (database) => {
    database.prepare(CREATE_R1_SCALARS_TABLE).run()
  },
  down: (database) => {
    database.prepare(`DROP TABLE scalar`).run()
  },
}

export default migration
