import type { MigrationDefinition } from "@dassie/lib-sqlite"

export const CREATE_R6_PEERS_TABLE = `
CREATE TABLE peers (
  node INTEGER PRIMARY KEY NOT NULL,
  settlement_scheme_id TEXT NOT NULL
) STRICT
`

const migration: MigrationDefinition = {
  version: 6,
  up: (database) => {
    database.prepare(CREATE_R6_PEERS_TABLE).run()
  },
  down: (database) => {
    database.prepare(`DROP TABLE peers`).run()
  },
}

export default migration
