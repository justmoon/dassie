import type SQLite from "better-sqlite3"

export interface MigrationDefinition {
  version: number
  up: (database: SQLite.Database) => void
  down: (database: SQLite.Database) => void
}

export { type default as SQLite } from "better-sqlite3"
