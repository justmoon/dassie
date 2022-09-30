import type { Database } from "better-sqlite3"

export interface MigrationDefinition {
  version: number
  up: (database: Database) => void
  down: (database: Database) => void
}
