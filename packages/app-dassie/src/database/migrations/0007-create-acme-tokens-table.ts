import type { MigrationDefinition } from "@dassie/lib-sqlite"

export const CREATE_R7_ACME_TOKENS_TABLE = `
CREATE TABLE acme_tokens (
  token TEXT PRIMARY KEY NOT NULL,
  key_authorization TEXT NOT NULL,
  expires TEXT
) STRICT
`

const migration: MigrationDefinition = {
  version: 7,
  up: (database) => {
    database.prepare(CREATE_R7_ACME_TOKENS_TABLE).run()
  },
  down: (database) => {
    database.prepare(`DROP TABLE acme_tokens`).run()
  },
}

export default migration
