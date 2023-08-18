import type { MigrationDefinition } from "@dassie/lib-sqlite"

const migration: MigrationDefinition = {
  version: 8,
  up: (database) => {
    database
      .prepare(
        `
          CREATE TABLE sessions (
            token TEXT PRIMARY KEY NOT NULL
          ) STRICT
        `
      )
      .run()
  },
  down: (database) => {
    database.prepare(`DROP TABLE sessions`).run()
  },
}

export default migration
