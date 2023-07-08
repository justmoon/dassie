import type { Simplify } from "type-fest"

import type {
  SqliteDataType,
  SqliteToTypescriptTypeMap,
} from "../../types/sqlite-datatypes"

export type InferColumnType<T extends ColumnDescription> =
  SqliteToTypescriptTypeMap[T["type"]]

export type InferRowFromColumns<TColumns extends readonly ColumnDescription[]> =
  Simplify<{
    [K in TColumns[number]["name"]]: InferColumnType<
      Extract<TColumns[number], { name: K }>
    >
  }>

export interface ColumnDescription {
  name: string
  type: SqliteDataType
}
