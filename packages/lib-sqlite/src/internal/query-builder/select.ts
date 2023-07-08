import type { Database, Statement } from "better-sqlite3"

import type { InferRowType, TableDescription } from "../../define-table"
import type { ColumnDescription, InferRowFromColumns } from "./columns"
import { createPlaceholderStore } from "./placeholders"
import { type Condition, generateWhereClause } from "./where"

export interface SelectQueryBuilder<TState extends SelectQueryBuilderState> {
  /**
   * Add a WHERE clause to the query.
   *
   * @remarks This method can be called multiple times in which case the conditions will be combined with AND.
   */
  where: (condition: Condition<TState["table"]>) => SelectQueryBuilder<TState>

  /**
   * Add a LIMIT clause to the query.
   *
   * If no offset is given, the offset is set to 0.
   */
  limit: (
    limit: number,
    offset?: number | undefined
  ) => SelectQueryBuilder<TState>

  /**
   * Generate the SQL for this query.
   */
  sql(): string

  /**
   * Generate the prepared SQLite Statement object for this query.
   */
  prepare(): Statement<[Record<string, unknown>]>

  /**
   * Execute the query and return all result rows.
   */
  all(): InferRowType<TState["table"]>[]

  /**
   * Execute the query and return the first result row.
   */
  first(): InferRowFromColumns<TState["columns"]> | undefined
}

export interface SelectQueryBuilderState {
  columns: readonly ColumnDescription[]
  table: TableDescription
}

export type NewSelectQueryBuilder<TTable extends TableDescription> =
  SelectQueryBuilder<{
    columns: TTable["columns"]
    table: TTable
  }>

export const createSelectQueryBuilder = <TTable extends TableDescription>(
  tableDescription: TTable,
  database: Database
): NewSelectQueryBuilder<TTable> => {
  const placeholders = createPlaceholderStore()
  const whereClauses: string[] = []
  const columns = "*"
  let limit: number | undefined
  let offset = 0

  let cachedStatement: Statement<unknown[]> | undefined

  const builder: NewSelectQueryBuilder<TTable> = {
    where(condition) {
      cachedStatement = undefined

      whereClauses.push(generateWhereClause(condition, placeholders))
      return builder
    },

    limit(newLimit: number, newOffset = 0) {
      cachedStatement = undefined

      limit = newLimit
      offset = newOffset
      return builder
    },

    sql() {
      const whereClause =
        whereClauses.length > 0
          ? `${whereClauses.map((clause) => `(${clause})`).join(" AND ")}`
          : // Always including the WHERE clause prevents ambiguity when using ON CONFLICT.
            // See: https://www.sqlite.org/lang_upsert.html#parsing_ambiguity (2.2 Parsing Ambiguity)
            "true"
      const limitClause = limit ? ` LIMIT ${limit} OFFSET ${offset}` : ""
      return `SELECT ${columns} FROM ${tableDescription.name} WHERE ${whereClause}${limitClause}`
    },

    prepare() {
      if (cachedStatement) return cachedStatement

      return (cachedStatement = database.prepare(builder.sql()))
    },

    all() {
      const query = builder.prepare()
      return query.all(placeholders.get()) as InferRowType<TTable>[]
    },

    first() {
      const query = builder.prepare()
      return query.get(placeholders.get()) as InferRowType<TTable> | undefined
    },
  }

  return builder
}
