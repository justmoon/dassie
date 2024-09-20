import type { MigrationDefinition } from "@dassie/lib-sqlite"

const migration: MigrationDefinition = {
  version: 1,
  up: (database) => {
    database
      .prepare(
        `
          CREATE TABLE scalar (
            -- String key used to index the value
            key TEXT PRIMARY KEY,

            -- Value (type is managed by the application)
            value ANY NOT NULL
          ) STRICT, WITHOUT ROWID
        `,
      )
      .run()
  },
  down: (database) => {
    database.prepare(`DROP TABLE scalar`).run()
  },
}

export default migration
