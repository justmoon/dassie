import type {
  ColumnDescription,
  InferRowFromColumns,
} from "./internal/query-builder/columns"

export interface TableDescription {
  name: string
  columns: Record<string, ColumnDescription>
}

export type InferRowType<T extends TableDescription> = InferRowFromColumns<
  T["columns"]
>

export const defineTable = <TTable extends TableDescription>(
  table: TTable
): TTable => table
