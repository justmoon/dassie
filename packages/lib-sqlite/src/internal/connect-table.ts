import type { Database } from "better-sqlite3"

import type { InferRowType, TableDescription } from "../define-table"
import type { InferColumnType } from "./query-builder/columns"
import {
  MultipleInsertResult,
  NewInsertQueryBuilder,
  SingleInsertResult,
  createInsertQueryBuilder,
} from "./query-builder/insert"
import {
  NewSelectQueryBuilder,
  createSelectQueryBuilder,
} from "./query-builder/select"

export interface ConnectedTable<TTable extends TableDescription> {
  /**
   * Create a new SELECT query builder.
   *
   * @see SelectQueryBuilder
   */
  select: () => NewSelectQueryBuilder<TTable>

  /**
   * Create a new INSERT query builder.
   *
   * @see InsertQueryBuilder
   */
  insert: () => NewInsertQueryBuilder<TTable>

  // --------------------------------------------------------------------------
  // Shorthand methods

  /**
   * Shorthand: Fetch all rows from the database.
   *
   * Equivalent to `table.select().all()`.
   */
  selectAll: () => InferRowType<TTable>[]

  /**
   * Shorthand: Fetch a unique row from the database.
   *
   * Equivalent to `table.select().where({ column: value }).first()`.
   */
  selectUnique: <TColumn extends keyof TTable["columns"] & string>(
    column: TColumn,
    value: InferColumnType<TTable["columns"][TColumn]>
  ) => InferRowType<TTable> | undefined

  /**
   * Shorthand: Insert a single row into the database.
   *
   * Equivalent to `table.insert().one(row)`.
   */
  insertOne: (row: InferRowType<TTable>) => SingleInsertResult

  /**
   * Shorthand: Insert multiple rows into the database.
   *
   * Equivalent to `table.insert().many(rows)`.
   */
  insertMany: (rows: InferRowType<TTable>[]) => MultipleInsertResult
}

export const connectTable = <TTable extends TableDescription>(
  tableDescription: TTable,
  database: Database
): ConnectedTable<TTable> => {
  const { name } = tableDescription

  const selectAllQuery = createSelectQueryBuilder(tableDescription, database)
  const insertQuery = createInsertQueryBuilder(tableDescription, database)

  return {
    select: () => {
      return createSelectQueryBuilder(tableDescription, database)
    },
    insert: () => {
      return createInsertQueryBuilder(tableDescription, database)
    },
    selectAll: () => {
      return selectAllQuery.all()
    },
    selectUnique: (column, value) => {
      const query = database.prepare(
        `SELECT * FROM ${name} WHERE ${column} = ?`
      )
      return query.get(value) as InferRowType<TTable>
    },
    insertOne: (row) => {
      return insertQuery.one(row)
    },
    insertMany: (rows) => {
      return insertQuery.many(rows)
    },
  }
}
