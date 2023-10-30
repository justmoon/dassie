import type { MigrationDefinition } from "@dassie/lib-sqlite"

const migration: MigrationDefinition = {
  version: 4,
  up: (database) => {
    database
      .prepare(
        `
          CREATE TABLE settlement_schemes (
            -- The unique settlement scheme ID
            id TEXT NOT NULL PRIMARY KEY,

            -- JSON representing the settlement scheme configuration
            -- The data format is specific to each settlement scheme module
            config TEXT NOT NULL
          ) STRICT
        `,
      )
      .run()
  },
  down: (database) => {
    database.prepare(`DROP TABLE settlement_schemes`).run()
  },
}

export default migration
