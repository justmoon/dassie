import type { MigrationDefinition } from "@dassie/lib-sqlite"

const migration: MigrationDefinition = {
  version: 9,
  up: (database) => {
    database
      .prepare(
        `
          CREATE TABLE btp_tokens (
            token TEXT PRIMARY KEY NOT NULL
          ) STRICT
        `,
      )
      .run()
  },
  down: (database) => {
    database.prepare(`DROP TABLE btp_tokens`).run()
  },
}

export default migration
