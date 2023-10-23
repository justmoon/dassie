import type { Database } from "better-sqlite3"
import { Simplify } from "type-fest"

import type { InferColumnTypescriptType } from "../types/column"
import type {
  InferColumnNames,
  InferRow,
  TableDescription,
} from "../types/table"
import {
  type MultipleInsertResult,
  type NewInsertQueryBuilder,
  type SingleInsertResult,
  createInsertQueryBuilder,
} from "./query-builder/insert"
import {
  type NewSelectQueryBuilder,
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
  selectAll: () => Simplify<InferRow<TTable>>[]

  /**
   * Shorthand: Fetch a unique row from the database.
   *
   * Equivalent to `table.select().where({ column: value }).first()`.
   */
  selectUnique: <TColumn extends InferColumnNames<TTable>>(
    column: TColumn,
    value: InferColumnTypescriptType<TTable["columns"][TColumn]>,
  ) => Simplify<InferRow<TTable>> | undefined

  /**
   * Shorthand: Insert a single row into the database.
   *
   * Equivalent to `table.insert().one(row)`.
   */
  insertOne: (row: Simplify<InferRow<TTable>>) => SingleInsertResult

  /**
   * Shorthand: Insert multiple rows into the database.
   *
   * Equivalent to `table.insert().many(rows)`.
   */
  insertMany: (rows: InferRow<TTable>[]) => MultipleInsertResult
}

export const connectTable = <TTable extends TableDescription>(
  tableDescription: TTable,
  database: Database,
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
        `SELECT * FROM ${name} WHERE ${column} = ?`,
      )
      return query.get(value) as InferRow<TTable>
    },
    insertOne: (row) => {
      return insertQuery.one(row)
    },
    insertMany: (rows) => {
      return insertQuery.many(rows)
    },
  }
}
