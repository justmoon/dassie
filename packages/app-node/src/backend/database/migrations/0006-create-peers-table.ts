import type { MigrationDefinition } from "@dassie/lib-sqlite"

const migration: MigrationDefinition = {
  version: 6,
  up: (database) => {
    database
      .prepare(
        `
          CREATE TABLE peers (
            node INTEGER PRIMARY KEY NOT NULL,
            settlement_scheme_id TEXT NOT NULL
          ) STRICT
        `
      )
      .run()
  },
  down: (database) => {
    database.prepare(`DROP TABLE peers`).run()
  },
}

export default migration
