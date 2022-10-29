import type { RequireExactlyOne } from "type-fest"

import type { InferRowType, TableDescription } from "../../define-table"
import type { PlaceholderStore } from "./placeholders"

export type Condition<TTable extends TableDescription> = RequireExactlyOne<
  ConditionTypes<TTable>
>

export interface ConditionTypes<TTable extends TableDescription> {
  equals: Partial<InferRowType<TTable>>
  and: Condition<TTable>[]
  or: Condition<TTable>[]
}

export const generateWhereClause = (
  condition: Condition<TableDescription>,
  mutablePlaceholders: PlaceholderStore
): string => {
  if ("equals" in condition) {
    const clauses = Object.entries(condition.equals).map(([column, value]) => {
      return `${column} = @${mutablePlaceholders.assign(value)}`
    })
    return clauses.join(" AND ")
  }

  if ("and" in condition) {
    const clauses = condition.and.map(
      (condition) => `(${generateWhereClause(condition, mutablePlaceholders)})`
    )
    return clauses.join(" AND ")
  }

  if ("or" in condition) {
    const clauses = condition.or.map(
      (condition) => `(${generateWhereClause(condition, mutablePlaceholders)})`
    )
    return clauses.join(" OR ")
  }

  throw new Error("Invalid condition")
}
