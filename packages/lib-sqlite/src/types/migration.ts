export interface MigrationDefinition {
  version: number
  up: (database: Database) => void
  down: (database: Database) => void
}

export interface Database {
  prepare(sql: string): SqliteStatement
  exec(sql: string): void
}

export interface SqliteStatement {
  run(): void
}
