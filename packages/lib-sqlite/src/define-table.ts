import type { Simplify } from "type-fest"

import type { InferTableDescription, TableOptions } from "./types/table"

export const table = <const TOptions extends TableOptions>(
  table: TOptions,
): Simplify<InferTableDescription<TOptions>> =>
  // eslint-disable-next-line @typescript-eslint/no-unsafe-return
  ({
    ...table,
    columns: Object.fromEntries(
      Object.entries(table.columns).map(([columnName, column]) => [
        columnName,
        column.description,
      ]),
    ),
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
  }) as any
