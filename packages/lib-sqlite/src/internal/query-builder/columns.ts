import type { Simplify } from "type-fest"

export type InferColumnType<T extends ColumnDescription> =
  SqliteToTypescriptTypeMap[T["type"]]

export type InferRowFromColumns<
  TColumns extends Record<string, ColumnDescription>
> = Simplify<{ [K in keyof TColumns]: InferColumnType<TColumns[K]> }>

export interface ColumnDescription {
  type: "TEXT" | "INTEGER" | "REAL" | "BOOLEAN" | "BLOB" | "ANY"
}

export interface SqliteToTypescriptTypeMap {
  TEXT: string
  INTEGER: bigint
  REAL: number
  BOOLEAN: boolean
  BLOB: Buffer
  ANY: unknown
}
