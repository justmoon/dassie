import type {
  ColumnDescription,
  InferRowFromColumns,
} from "./internal/query-builder/columns"

export interface TableDescription {
  name: string
  columns: readonly ColumnDescription[]
}

export type InferRowType<T extends TableDescription> = InferRowFromColumns<
  T["columns"]
>

export type InferColumnNames<T extends TableDescription> =
  T["columns"][number]["name"] & string

export const defineTable = <const TTable extends TableDescription>(
  table: TTable
): TTable => table
