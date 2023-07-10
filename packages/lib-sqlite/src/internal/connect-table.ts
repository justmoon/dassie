import type { Database } from "better-sqlite3"
import { Simplify } from "type-fest"

import type { InferColumnReadType } from "../types/column"
import type {
  AnyTableDescription,
  InferColumnNames,
  InferRowReadType,
  InferRowSqliteType,
  InferRowWriteType,
  TableDescription,
  TableDescriptionGenerics,
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
import {
  createRowDeserializer,
  createRowSerializer,
} from "./query-builder/serialize"

export interface ConnectedTable<
  TTable extends TableDescription<TableDescriptionGenerics>
> {
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
  selectAll: () => Simplify<InferRowReadType<TTable>>[]

  /**
   * Shorthand: Fetch a unique row from the database.
   *
   * Equivalent to `table.select().where({ column: value }).first()`.
   */
  selectUnique: <TColumn extends InferColumnNames<TTable>>(
    column: TColumn,
    value: InferColumnReadType<TTable["columns"][TColumn]["description"]>
  ) => Simplify<InferRowReadType<TTable>> | undefined

  /**
   * Shorthand: Insert a single row into the database.
   *
   * Equivalent to `table.insert().one(row)`.
   */
  insertOne: (row: InferRowWriteType<TTable>) => SingleInsertResult

  /**
   * Shorthand: Insert multiple rows into the database.
   *
   * Equivalent to `table.insert().many(rows)`.
   */
  insertMany: (rows: InferRowWriteType<TTable>[]) => MultipleInsertResult
}

export const connectTable = <TTable extends AnyTableDescription>(
  tableDescription: TTable,
  database: Database
): ConnectedTable<TTable> => {
  const { name } = tableDescription

  const serializeRow = createRowSerializer(tableDescription)
  const deserializeRow = createRowDeserializer(tableDescription)

  const selectAllQuery = createSelectQueryBuilder(
    tableDescription,
    deserializeRow,
    database
  )
  const insertQuery = createInsertQueryBuilder(
    tableDescription,
    serializeRow,
    database
  )

  return {
    select: () => {
      return createSelectQueryBuilder(
        tableDescription,
        deserializeRow,
        database
      )
    },
    insert: () => {
      return createInsertQueryBuilder(tableDescription, serializeRow, database)
    },
    selectAll: () => {
      return selectAllQuery.all()
    },
    selectUnique: (column, value) => {
      const query = database.prepare(
        `SELECT * FROM ${name} WHERE ${column} = ?`
      )
      return deserializeRow(query.get(value) as InferRowSqliteType<TTable>)
    },
    insertOne: (row) => {
      return insertQuery.one(row)
    },
    insertMany: (rows) => {
      return insertQuery.many(rows)
    },
  }
}
