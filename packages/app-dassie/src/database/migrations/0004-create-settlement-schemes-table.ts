import type { MigrationDefinition } from "@dassie/lib-sqlite"

export const CREATE_R4_SETTLEMENT_SCHEMES_TABLE = `
CREATE TABLE settlement_schemes (
  -- The unique settlement scheme ID
  id TEXT NOT NULL PRIMARY KEY,

  -- JSON representing the settlement scheme configuration
  -- The data format is specific to each settlement scheme module
  config TEXT NOT NULL
) STRICT
`
const migration: MigrationDefinition = {
  version: 4,
  up: (database) => {
    database.prepare(CREATE_R4_SETTLEMENT_SCHEMES_TABLE).run()
  },
  down: (database) => {
    database.prepare(`DROP TABLE settlement_schemes`).run()
  },
}

export default migration
