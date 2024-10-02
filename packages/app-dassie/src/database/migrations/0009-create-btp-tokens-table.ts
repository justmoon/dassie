import type { MigrationDefinition } from "@dassie/lib-sqlite"

export const CREATE_R9_BTP_TOKENS_TABLE = `
CREATE TABLE btp_tokens (
  token TEXT PRIMARY KEY NOT NULL
) STRICT
`

const migration: MigrationDefinition = {
  version: 9,
  up: (database) => {
    database.prepare(CREATE_R9_BTP_TOKENS_TABLE).run()
  },
  down: (database) => {
    database.prepare(`DROP TABLE btp_tokens`).run()
  },
}

export default migration
