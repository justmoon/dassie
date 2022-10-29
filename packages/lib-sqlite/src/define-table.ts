import type {
  ColumnDescription,
  InferRowFromColumns,
} from "./internal/query-builder/columns"
import type { ScalarDescription } from "./internal/scalar-store"

export interface TableDescription {
  name: string
  columns: Record<string, ColumnDescription>
  scalars?: Record<string, ScalarDescription>
}

export type InferRowType<T extends TableDescription> = InferRowFromColumns<
  T["columns"]
>

export const defineTable = <TTable extends TableDescription>(
  table: TTable
): TTable => table
