import type { Database } from "better-sqlite3"
import type { Simplify } from "type-fest"

export interface TableDescription {
  name: string
  columns: Record<string, ColumnDescription>
}

export interface ColumnDescription {
  type: "string" | "integer" | "boolean" | "float" | "blob"
}

export interface SqliteToTypescriptTypeMap {
  string: string
  integer: bigint
  boolean: boolean
  float: number
  blob: Buffer
}

export type InferColumnType<T extends ColumnDescription> =
  SqliteToTypescriptTypeMap[T["type"]]

export type InferRowType<T extends TableDescription> = Simplify<{
  [K in keyof T["columns"]]: InferColumnType<T["columns"][K]>
}>

export interface ConnectedTable<TTable extends TableDescription> {
  select: () => InferRowType<TTable>[]
  selectFirst: <TColumn extends keyof TTable["columns"] & string>(
    column: TColumn,
    value: InferColumnType<TTable["columns"][TColumn]>
  ) => InferRowType<TTable> | undefined
  insert: (row: InferRowType<TTable>) => bigint
  insertMany: (rows: InferRowType<TTable>[]) => bigint[]
}

export interface StaticTable<TTable extends TableDescription> {
  name: string
  connect: (database: Database) => ConnectedTable<TTable>
}

export const defineTable = <TTable extends TableDescription>(
  table: TTable
): TTable => {
  return table
}

export const connectTable = <TTable extends TableDescription>(
  tableDescription: TTable,
  database: Database
): ConnectedTable<TTable> => {
  const { name, columns } = tableDescription

  const insertQuery = database.prepare(
    `INSERT INTO ${name} (${Object.keys(columns).join(
      ", "
    )}) VALUES (${Object.keys(columns)
      .map((columnName) => `@${columnName}`)
      .join(", ")})`
  )

  return {
    select: () => {
      const query = database.prepare(`SELECT * FROM ${name}`)
      return query.all() as InferRowType<TTable>[]
    },
    selectFirst: (column, value) => {
      const query = database.prepare(
        `SELECT * FROM ${name} WHERE ${column} = ?`
      )
      return query.get(value) as InferRowType<TTable>
    },
    insert: (row) => {
      return insertQuery.run(row).lastInsertRowid
    },
    insertMany: database.transaction<
      (rows: InferRowType<TTable>[]) => bigint[]
    >((rows) => {
      return rows.map((row) => insertQuery.run(row).lastInsertRowid)
    }),
  }
}
