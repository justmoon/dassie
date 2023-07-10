import type { Database, RunResult, Statement } from "better-sqlite3"

import type {
  InferColumnNames,
  InferRowWriteType,
  TableDescription,
  TableDescriptionGenerics,
} from "../../types/table"
import { RowSerializer } from "./serialize"

export interface InsertQueryBuilder<TState extends InsertQueryBuilderState> {
  /**
   * Ignore any errors that occur due to duplicate unique keys.
   *
   * @remarks
   *
   * Will throw an error if `ignoreConflicts` or `upsert` has been called previously.
   *
   * @param indexedColumns - The "conflict target" columns. These are columns with UNIQUE or PRIMARY KEY constraints that should be ignored when inserting.
   */
  ignoreConflicts(
    indexedColumns: readonly InferColumnNames<TState["table"]>[]
  ): InsertQueryBuilder<TState>

  /**
   * Update the row if it already exists.
   *
   * @remarks
   *
   * Will throw an error if `ignoreConflicts` or `upsert` has been called previously.
   *
   * @param indexedColumns - The "conflict target" columns. These are columns with UNIQUE or PRIMARY KEY constraints that will trigger an update if they conflict.
   * @param updateColumns - The columns that should be updated if a conflict occurs.
   */
  upsert(
    indexedColumns: readonly InferColumnNames<TState["table"]>[],
    updateColumns: readonly InferColumnNames<TState["table"]>[]
  ): InsertQueryBuilder<TState>

  /**
   * Generate the SQL for this query.
   */
  sql(): string

  /**
   * Generate the prepared SQLite Statement object for this query.
   */
  prepare(): Statement<unknown[]>

  /**
   * Insert a single row and return the rowid of the inserted row.
   */
  one(row: InferRowWriteType<TState["table"]>): SingleInsertResult

  /**
   * Insert multiple rows in a single transaction and return the rowids of the inserted rows.
   */
  many(
    rows: readonly InferRowWriteType<TState["table"]>[]
  ): MultipleInsertResult
}

export interface InsertQueryBuilderState {
  table: TableDescription<TableDescriptionGenerics>
}

export type NewInsertQueryBuilder<
  TTable extends TableDescription<TableDescriptionGenerics>
> = InsertQueryBuilder<{
  table: TTable
}>

export interface SingleInsertResult {
  changes: 0 | 1
  rowid: bigint
}

export interface MultipleInsertResult {
  changes: number
  rowids: bigint[]
}

export const createInsertQueryBuilder = <
  TTable extends TableDescription<TableDescriptionGenerics>
>(
  tableDescription: TTable,
  serializeRow: RowSerializer<TTable>,
  database: Database
): NewInsertQueryBuilder<TTable> => {
  let cachedStatement: Statement<unknown[]> | undefined
  let indexedColumns: readonly InferColumnNames<TTable>[] | undefined
  let updateColumns: readonly InferColumnNames<TTable>[] | undefined

  const builder: NewInsertQueryBuilder<TTable> = {
    ignoreConflicts(newIndexedColumns) {
      cachedStatement = undefined

      if (indexedColumns !== undefined) {
        throw new Error(
          "Cannot call ignoreConflicts() or upsert() multiple times."
        )
      }

      indexedColumns = newIndexedColumns

      return builder
    },

    upsert(newIndexedColumns, newUpdateColumns) {
      cachedStatement = undefined

      if (indexedColumns !== undefined) {
        throw new Error(
          "Cannot call ignoreConflicts() or upsert() multiple times."
        )
      }

      indexedColumns = newIndexedColumns
      updateColumns = newUpdateColumns

      return builder
    },

    sql() {
      const conflictClause = indexedColumns
        ? updateColumns
          ? "ON CONFLICT (" +
            indexedColumns.map((column) => `"${column}"`).join(", ") +
            ") DO UPDATE SET " +
            updateColumns
              .map((column) => `"${column}" = excluded."${column}"`)
              .join(", ")
          : "ON CONFLICT (" +
            indexedColumns.map((column) => `"${column}"`).join(", ") +
            ") DO NOTHING"
        : ""

      return `INSERT INTO ${tableDescription.name} (${Object.keys(
        tableDescription.columns
      ).join(", ")}) VALUES (${Object.keys(tableDescription.columns)
        .map((column) => `@${column}`)
        .join(", ")})${conflictClause}`
    },

    prepare() {
      if (cachedStatement) return cachedStatement

      return (cachedStatement = database.prepare(builder.sql()))
    },

    one(row) {
      const query = builder.prepare()
      const { changes, lastInsertRowid } = query.run(serializeRow(row))

      return {
        changes: changes as 0 | 1,
        rowid: lastInsertRowid,
      }
    },

    many(rows) {
      const query = builder.prepare()
      const results = database.transaction<(_rows: typeof rows) => RunResult[]>(
        (rows) => {
          return rows.map((row) => query.run(serializeRow(row)))
        }
      )(rows)

      let changes = 0
      const rowids: bigint[] = []

      for (const result of results) {
        changes += result.changes
        rowids.push(result.lastInsertRowid)
      }

      return { changes, rowids }
    },
  }

  return builder
}
