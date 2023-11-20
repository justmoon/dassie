import type { MigrationDefinition } from "@dassie/lib-sqlite"

const migration: MigrationDefinition = {
  version: 10,
  up: (database) => {
    database
      .prepare(
        `ALTER TABLE peers ADD COLUMN settlement_scheme_state TEXT NOT NULL DEFAULT "{}"`,
      )
      .run()
  },
  down: (database) => {
    database
      .prepare(`ALTER TABLE peers DROP COLUMN settlement_scheme_state`)
      .run()
  },
}

export default migration
