import type { MigrationDefinition } from "@dassie/lib-sqlite"

export const CREATE_R13_REGISTRATIONS_TABLE = `
CREATE TABLE registrations (
  node INTEGER PRIMARY KEY NOT NULL,
  registered_at INTEGER NOT NULL,
  renewed_at INTEGER NOT NULL
) STRICT
`

const migration: MigrationDefinition = {
  version: 13,
  up: (database) => {
    database.prepare(CREATE_R13_REGISTRATIONS_TABLE).run()
  },
  down: (database) => {
    database.prepare(`DROP TABLE registrations`).run()
  },
}

export default migration
