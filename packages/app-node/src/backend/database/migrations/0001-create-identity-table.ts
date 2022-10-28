import type { MigrationDefinition } from "@dassie/lib-sqlite"

const migration: MigrationDefinition = {
  version: 1,
  up: (database) => {
    database
      .prepare(
        `
          CREATE TABLE identity (
            -- There must not be more than one row in this table
            id INTEGER PRIMARY KEY CHECK (id = 0),

            -- Node cryptographic seed
            seed BLOB NOT NULL
          )
        `
      )
      .run()
  },
  down: (database) => {
    database.prepare(`DROP TABLE identity`).run()
  },
}

export default migration
