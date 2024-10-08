import type SQLite from "better-sqlite3"

import type { TableDescription } from "../types/table"

interface SqliteTableSchema {
  schema: string
  name: string
  type: string
  ncol: bigint
  wr: bigint
  strict: bigint
}

interface SqliteColumnSchema {
  cid: bigint
  name: string
  type: string
  notnull: 0n | 1n
  dflt_value: string | null
  pk: 0n | 1n
}

export const checkSchema = (
  database: SQLite.Database,
  tables: Record<string, TableDescription>,
) => {
  const escapeValue = (value: unknown): string =>
    database.prepare("SELECT CAST(? AS TEXT)").pluck().get(value) as string

  const actualTables = Object.fromEntries(
    (database.pragma("table_list") as SqliteTableSchema[]).map((table) => [
      table.name,
      table,
    ]),
  )

  for (const table of Object.values(tables)) {
    const actualColumns = database.pragma(
      `table_info(${table.name})`,
    ) as SqliteColumnSchema[]

    if (actualColumns.length === 0) {
      throw new Error(`Table "${table.name}" not found`)
    }

    if (actualTables[table.name]?.strict !== 1n) {
      throw new Error(`Table "${table.name}" should be marked STRICT but isn't`)
    }

    if ((actualTables[table.name]?.wr === 1n) !== !!table.withoutRowid) {
      throw new Error(
        table.withoutRowid ?
          `Table "${table.name}" should be WITHOUT ROWID but isn't`
        : `Table "${table.name}" should not be WITHOUT ROWID but is`,
      )
    }

    for (const [columnName, expectedSchema] of Object.entries(table.columns)) {
      const actualSchema = actualColumns.find(
        (schema) => schema.name === columnName,
      )

      if (!actualSchema) {
        throw new Error(
          `Column "${columnName}" not found in table "${table.name}"`,
        )
      }

      if (actualSchema.type !== expectedSchema.type) {
        throw new Error(
          `Column "${columnName}" in table "${table.name}" has type "${
            actualSchema.type
          }" but should be "${expectedSchema.type as string}"`,
        )
      }

      // We require all tables to be in STRICT mode in which case any column marked PRIMARY KEY will also be NOT NULL
      const expectNotNull = expectedSchema.notNull || expectedSchema.primaryKey
      if (expectNotNull && actualSchema.notnull !== 1n) {
        throw new Error(
          `Column "${columnName}" in table "${table.name}" should be marked NOT NULL but isn't`,
        )
      } else if (!expectNotNull && actualSchema.notnull !== 0n) {
        throw new Error(
          `Column "${columnName}" in table "${table.name}" should not be marked NOT NULL but is`,
        )
      }

      if (expectedSchema.primaryKey && actualSchema.pk !== 1n) {
        throw new Error(
          `Column "${columnName}" in table "${table.name}" should be marked PRIMARY KEY but isn't`,
        )
      } else if (!expectedSchema.primaryKey && actualSchema.pk !== 0n) {
        throw new Error(
          `Column "${columnName}" in table "${table.name}" should not be marked PRIMARY KEY but is`,
        )
      }

      if (expectedSchema.hasDefault && actualSchema.dflt_value === null) {
        throw new Error(
          `Column "${columnName}" in table "${table.name}" should have a default value but doesn't`,
        )
      } else if (
        expectedSchema.hasDefault &&
        actualSchema.dflt_value !== escapeValue(expectedSchema.defaultValue)
      ) {
        throw new Error(
          `Column "${columnName}" in table "${table.name}" should have a default value of "${escapeValue(expectedSchema.defaultValue)}" but has "${actualSchema.dflt_value}"`,
        )
      }
    }
  }
}
