import type Database from "better-sqlite3"

import { TableDescription, TableDescriptionGenerics } from "../types/table"

interface ColumnSchema {
  cid: bigint
  name: string
  type: string
  notnull: 0n | 1n
  dflt_value: string | null
  pk: 0n | 1n
}

export const checkSchema = (
  database: Database.Database,
  tables: Record<string, TableDescription<TableDescriptionGenerics>>
) => {
  for (const table of Object.values(tables)) {
    const tableSchema = database.pragma(
      `table_info(${table.name})`
    ) as ColumnSchema[]

    if (tableSchema.length === 0) {
      throw new Error(`Table "${table.name}" not found`)
    }

    for (const [columnName, expectedSchema] of Object.entries(table.columns)) {
      const actualSchema = tableSchema.find(
        (schema) => schema.name === columnName
      )

      if (!actualSchema) {
        throw new Error(
          `Column "${columnName}" not found in table "${table.name}"`
        )
      }

      if (actualSchema.type !== expectedSchema.description.type) {
        throw new Error(
          `Column "${columnName}" in table "${table.name}" has type "${
            actualSchema.type
          }" but should be "${expectedSchema.description.type as string}"`
        )
      }

      if (expectedSchema.description.required && actualSchema.notnull !== 1n) {
        throw new Error(
          `Column "${columnName}" in table "${table.name}" should be marked NOT NULL but isn't`
        )
      } else if (
        !expectedSchema.description.required &&
        actualSchema.notnull !== 0n
      ) {
        throw new Error(
          `Column "${columnName}" in table "${table.name}" should not be marked NOT NULL but is`
        )
      }

      if (expectedSchema.description.primaryKey && actualSchema.pk !== 1n) {
        throw new Error(
          `Column "${columnName}" in table "${table.name}" should be marked PRIMARY KEY but isn't`
        )
      } else if (
        !expectedSchema.description.primaryKey &&
        actualSchema.pk !== 0n
      ) {
        throw new Error(
          `Column "${columnName}" in table "${table.name}" should not be marked PRIMARY KEY but is`
        )
      }
    }
  }
}
