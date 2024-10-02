import type { MigrationDefinition } from "@dassie/lib-sqlite"

export const CREATE_R11_ACCOUNTS_TABLE = `
CREATE TABLE accounts (
  path TEXT PRIMARY KEY NOT NULL,
  debits_posted INTEGER NOT NULL DEFAULT 0,
  credits_posted INTEGER NOT NULL DEFAULT 0
) STRICT
`

const migration: MigrationDefinition = {
  version: 11,
  up: (database) => {
    database.prepare(CREATE_R11_ACCOUNTS_TABLE).run()
  },
  down: (database) => {
    database.prepare(`DROP TABLE accounts`).run()
  },
}

export default migration
