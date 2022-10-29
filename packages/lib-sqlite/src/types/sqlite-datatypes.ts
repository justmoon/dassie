export type SqliteDataType =
  | "TEXT"
  | "INTEGER"
  | "REAL"
  | "BOOLEAN"
  | "BLOB"
  | "ANY"

export interface SqliteToTypescriptTypeMap {
  TEXT: string
  INTEGER: bigint
  REAL: number
  BOOLEAN: boolean
  BLOB: Buffer
  ANY: unknown
}
