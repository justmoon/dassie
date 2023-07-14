import type { MigrationDefinition } from "@dassie/lib-sqlite"

const migration: MigrationDefinition = {
  version: 5,
  up: (database) => {
    database
      .prepare(
        `
          CREATE TABLE nodes (
            id TEXT NOT NULL PRIMARY KEY,
            public_key BLOB NOT NULL,
            url TEXT NOT NULL,
            alias TEXT NOT NULL
          ) STRICT
        `
      )
      .run()
  },
  down: (database) => {
    database.prepare(`DROP TABLE nodes`).run()
  },
}

export default migration
