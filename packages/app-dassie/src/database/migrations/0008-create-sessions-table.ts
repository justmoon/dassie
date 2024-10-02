import type { MigrationDefinition } from "@dassie/lib-sqlite"

export const CREATE_R8_SESSIONS_TABLE = `
CREATE TABLE sessions (
  token TEXT PRIMARY KEY NOT NULL
) STRICT
`

const migration: MigrationDefinition = {
  version: 8,
  up: (database) => {
    database.prepare(CREATE_R8_SESSIONS_TABLE).run()
  },
  down: (database) => {
    database.prepare(`DROP TABLE sessions`).run()
  },
}

export default migration
