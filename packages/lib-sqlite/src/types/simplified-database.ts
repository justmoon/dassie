export interface PragmaOptions {
  simple?: boolean | undefined
}

export interface SqliteStatement {
  run(): void
}

export interface SimplifiedDatabase {
  memory: boolean
  readonly: boolean
  name: string
  open: boolean
  inTransaction: boolean

  pragma(sql: string): void
  prepare(sql: string): SqliteStatement
  exec(sql: string): void
}
