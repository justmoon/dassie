import type { MigrationDefinition } from "@dassie/lib-sqlite"

export const CREATE_R5_NODES_TABLE = `
CREATE TABLE nodes (
  id TEXT NOT NULL PRIMARY KEY,
  public_key BLOB NOT NULL,
  url TEXT NOT NULL,
  alias TEXT NOT NULL
) STRICT
`

const migration: MigrationDefinition = {
  version: 5,
  up: (database) => {
    database.prepare(CREATE_R5_NODES_TABLE).run()
  },
  down: (database) => {
    database.prepare(`DROP TABLE nodes`).run()
  },
}

export default migration
