import type { Simplify } from "type-fest"

import type {
  SqliteDataType,
  SqliteToTypescriptTypeMap,
} from "../../types/sqlite-datatypes"

export type InferColumnType<T extends ColumnDescription> =
  SqliteToTypescriptTypeMap[T["type"]]

export type InferRowFromColumns<
  TColumns extends Record<string, ColumnDescription>
> = Simplify<{ [K in keyof TColumns]: InferColumnType<TColumns[K]> }>

export interface ColumnDescription {
  type: SqliteDataType
}
