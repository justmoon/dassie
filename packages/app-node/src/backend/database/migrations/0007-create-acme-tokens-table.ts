import type { MigrationDefinition } from "@dassie/lib-sqlite"

const migration: MigrationDefinition = {
  version: 7,
  up: (database) => {
    database
      .prepare(
        `
          CREATE TABLE acme_tokens (
            token TEXT PRIMARY KEY NOT NULL,
            key_authorization TEXT NOT NULL,
            expires TEXT
          ) STRICT
        `,
      )
      .run()
  },
  down: (database) => {
    database.prepare(`DROP TABLE acme_tokens`).run()
  },
}

export default migration
