export type SqliteDataType = "TEXT" | "INTEGER" | "REAL" | "BLOB" | "ANY"

export interface SqliteToTypescriptTypeMap {
  TEXT: string
  INTEGER: bigint
  REAL: number
  BLOB: Buffer
  ANY: NonNullable<unknown>
}
