import { AnyTableDescription } from "./types/table"

export const table = <const TTable extends AnyTableDescription>(
  table: TTable
): TTable => table
