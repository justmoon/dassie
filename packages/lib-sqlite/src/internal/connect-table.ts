import type { Database, RunResult } from "better-sqlite3"
import type { Simplify } from "type-fest"

import type {
  InferInsertRow,
  InferRow,
  InferRowWithRowid,
  TableDescription,
} from "../types/table"

export type BigIntRunResult = RunResult & {
  lastInsertRowid: bigint
}

export interface InsertOptions {
  ignoreConflicts?: boolean
}

export interface InsertManyResult {
  changes: number
  rowids: bigint[]
}

/**
 * Provides useful shorthands for common database operations.
 *
 * For more advanced queries, use Kysely.
 */
export interface ConnectedTable<TTable extends TableDescription> {
  /**
   * Select rows from the database.
   *
   * @param row - An object which will be used to filter the rows.
   * @returns An array of rows matching the given criteria.
   */
  select: (
    row?: Partial<InferRowWithRowid<TTable>>,
  ) => Simplify<InferRowWithRowid<TTable>>[]

  /**
   * Fetch all rows from the database.
   *
   * @returns An array of all rows in the table.
   */
  selectAll: () => Simplify<InferRowWithRowid<TTable>>[]

  /**
   * Fetch a unique row from the database.
   *
   * @param row - An object which will be used to filter the rows.
   * @returns The first row matching the given criteria or undefined if no rows match.
   */
  selectFirst: (
    row?: Partial<InferRowWithRowid<TTable>>,
  ) => Simplify<InferRowWithRowid<TTable>> | undefined

  /**
   * Update rows matching a set of fields.
   */
  update: (
    conditions: Partial<InferRowWithRowid<TTable>>,
    newValues: Partial<InferRow<TTable>>,
  ) => BigIntRunResult

  /**
   * Update all rows in the table.
   *
   * @param newValues - A record of the fields to update and their new values.
   */
  updateAll: (newValues: Partial<InferRow<TTable>>) => BigIntRunResult

  /**
   * Insert a single row into the database.
   *
   * @param row - The row to insert.
   */
  insertOne: (
    row: Simplify<InferInsertRow<TTable>>,
    options?: InsertOptions,
  ) => BigIntRunResult

  /**
   * Insert multiple rows into the database.
   *
   * @remarks
   *
   * This is more efficient than calling insertOne() multiple times.
   *
   * @param rows - The rows to insert.
   */
  insertMany: (
    rows: InferInsertRow<TTable>[],
    options?: InsertOptions,
  ) => InsertManyResult

  /**
   * Delete rows matching a set of fields.
   *
   * @remarks
   *
   * Unlike select(), the parameter is required to prevent accidental deletion
   * of all rows.
   *
   * @param row - An object which will be used to filter the rows.
   */
  delete: (row: Partial<InferRowWithRowid<TTable>>) => BigIntRunResult

  /**
   * Delete all rows from the table.
   */
  deleteAll: () => BigIntRunResult

  /**
   * Inserts a row, or - in the case of a conflicting row - updates it.
   *
   * @param row - The row to insert or update.
   * @param conflictColumns - The columns to check for conflicts.
   */
  upsert: (
    row: Simplify<InferInsertRow<TTable>>,
    conflictColumns: (keyof InferRow<TTable>)[],
  ) => BigIntRunResult

  /**
   * Iterate over all rows in the table.
   *
   * @example
   *
   * ```
   * for (const user of database.tables.users) {
   *   if (user.level === "admin") {
   *     console.log(user.name)
   *   }
   * }
   * ```
   */
  [Symbol.iterator]: () => IterableIterator<Simplify<InferRow<TTable>>>
}

export const connectTable = <TTable extends TableDescription>(
  tableDescription: TTable,
  database: Database,
): ConnectedTable<TTable> => {
  const { name, columns, withoutRowid } = tableDescription

  const allColumns = withoutRowid ? "*" : "rowid, *"
  const selectAllQuery = database.prepare(`SELECT ${allColumns} FROM ${name}`)

  const createWhereClause = (
    row: Partial<InferRowWithRowid<TTable>>,
    prefix = "",
  ) => {
    const keys = Object.keys(row)

    if (keys.length === 0) {
      throw new Error("Empty condition object")
    }

    return Object.keys(row)
      .map((key) => `${key} = @${prefix}${key}`)
      .join(" AND ")
  }

  const createUpdateClause = (row: Partial<InferRow<TTable>>, prefix = "") => {
    const keys = Object.keys(row)

    if (keys.length === 0) {
      throw new Error("Empty update clause")
    }

    return Object.keys(row)
      .map((key) => `${key} = @${prefix}${key}`)
      .join(", ")
  }

  const createInsertQuery = (columns: string[], options?: InsertOptions) =>
    database.prepare(
      `INSERT ${options?.ignoreConflicts ? "OR IGNORE " : ""}INTO ${name} (${columns.join(", ")}) VALUES (${columns
        .map((key) => `@${key}`)
        .join(", ")})`,
    )

  /**
   * Prefix all keys in the given record with a given string prefix.
   */
  const prefixKeys = (
    row: Partial<InferRowWithRowid<TTable>> | Partial<InferRow<TTable>>,
    prefix: string,
  ) => {
    return Object.fromEntries(
      Object.entries(row).map(([key, value]) => [`${prefix}${key}`, value]),
    )
  }

  return {
    select: (row) => {
      if (!row) {
        return selectAllQuery.all() as Simplify<InferRowWithRowid<TTable>>[]
      }

      const query = database.prepare<[typeof row]>(
        `SELECT * FROM ${name} WHERE ${createWhereClause(row)}`,
      )

      return query.all(row) as Simplify<InferRowWithRowid<TTable>>[]
    },
    selectAll: () => {
      return selectAllQuery.all() as Simplify<InferRowWithRowid<TTable>>[]
    },
    selectFirst: (row) => {
      const query =
        row ?
          database.prepare(
            `SELECT * FROM ${name} WHERE ${createWhereClause(row)}`,
          )
        : selectAllQuery

      return query.get(row) as Simplify<InferRowWithRowid<TTable>> | undefined
    },
    update: (conditions, newValues) => {
      const query = database.prepare(
        `UPDATE ${name} SET ${createUpdateClause(
          newValues,
          "u_",
        )} WHERE ${createWhereClause(conditions, "w_")}`,
      )

      return query.run({
        ...prefixKeys(conditions, "w_"),
        ...prefixKeys(newValues, "u_"),
      }) as BigIntRunResult
    },
    updateAll: (newValues) => {
      const query = database.prepare(
        `UPDATE ${name} SET ${createUpdateClause(newValues)}`,
      )

      return query.run(newValues) as BigIntRunResult
    },
    insertOne: (row, options) => {
      const insertQuery = createInsertQuery(Object.keys(row), options)
      return insertQuery.run(row) as BigIntRunResult
    },
    insertMany: (rows, options) => {
      if (rows.length === 0) {
        return { changes: 0, rowids: [] }
      }

      const insertQuery = createInsertQuery(Object.keys(rows[0]!), options)
      const results = database.transaction<(_rows: typeof rows) => RunResult[]>(
        (rows) => {
          return rows.map((row) => insertQuery.run(row))
        },
      )(rows)

      let changes = 0
      const rowids: bigint[] = []

      for (const result of results) {
        changes += result.changes
        rowids.push(BigInt(result.lastInsertRowid))
      }

      return { changes, rowids }
    },
    delete: (row) => {
      const query = database.prepare<[typeof row]>(
        `DELETE FROM ${name} WHERE ${Object.keys(row)
          .map((key) => `${key} = @${key}`)
          .join(" AND ")}`,
      )

      return query.run(row) as BigIntRunResult
    },
    deleteAll: () => {
      return database.prepare(`DELETE FROM ${name}`).run() as BigIntRunResult
    },
    upsert: (row, conflictColumns) => {
      const keys = Object.keys(row)
      const columnList = Object.keys(columns).join(", ")
      const querySource = `INSERT INTO ${name} (${columnList}) VALUES (${keys
        .map((key) => `@${key}`)
        .join(", ")}) ON CONFLICT (${conflictColumns.join(
        ", ",
      )}) DO UPDATE SET ${keys
        .map((key) => `${key} = EXCLUDED.${key}`)
        .join(", ")}`

      const query = database.prepare(querySource)

      return query.run(row) as BigIntRunResult
    },
    [Symbol.iterator]: () => {
      const query = database.prepare(`SELECT * FROM ${name}`)

      return query.iterate() as IterableIterator<Simplify<InferRow<TTable>>>
    },
  }
}
