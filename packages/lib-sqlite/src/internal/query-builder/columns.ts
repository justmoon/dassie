import type { Simplify } from "type-fest"

export type InferColumnType<T extends ColumnDescription> =
  SqliteToTypescriptTypeMap[T["type"]]

export type InferRowFromColumns<
  TColumns extends Record<string, ColumnDescription>
> = Simplify<{ [K in keyof TColumns]: InferColumnType<TColumns[K]> }>

export interface ColumnDescription {
  type: "string" | "integer" | "boolean" | "float" | "blob"
}

export interface SqliteToTypescriptTypeMap {
  string: string
  integer: bigint
  boolean: boolean
  float: number
  blob: Buffer
}
