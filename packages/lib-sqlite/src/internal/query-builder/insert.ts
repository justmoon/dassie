import type { Database, RunResult, Statement } from "better-sqlite3"

import type { InferRowType, TableDescription } from "../../define-table"

export interface InsertQueryBuilder<TState extends InsertQueryBuilderState> {
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
  one(row: InferRowType<TState["table"]>): SingleInsertResult

  /**
   * Insert multiple rows in a single transaction and return the rowids of the inserted rows.
   */
  many(rows: readonly InferRowType<TState["table"]>[]): MultipleInsertResult
}

export interface InsertQueryBuilderState {
  table: TableDescription
}

export type NewInsertQueryBuilder<TTable extends TableDescription> =
  InsertQueryBuilder<{
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

export const createInsertQueryBuilder = <TTable extends TableDescription>(
  tableDescription: TTable,
  database: Database
): NewInsertQueryBuilder<TTable> => {
  let cachedStatement: Statement<unknown[]> | undefined

  const builder: NewInsertQueryBuilder<TTable> = {
    sql() {
      return `INSERT INTO ${tableDescription.name} (${Object.keys(
        tableDescription.columns
      ).join(", ")}) VALUES (${Object.keys(tableDescription.columns)
        .map((columnName) => `@${columnName}`)
        .join(", ")})`
    },

    prepare() {
      if (cachedStatement) return cachedStatement

      return (cachedStatement = database.prepare(builder.sql()))
    },

    one(row) {
      const query = builder.prepare()
      const { changes, lastInsertRowid } = query.run(row)

      return {
        changes: changes as 0 | 1,
        rowid: lastInsertRowid,
      }
    },

    many(rows) {
      const query = builder.prepare()
      const results = database.transaction<(_rows: typeof rows) => RunResult[]>(
        (rows) => {
          return rows.map((row) => query.run(row))
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
