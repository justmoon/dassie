import Database from "better-sqlite3"

import { createLogger } from "@dassie/lib-logger"

import { ConnectedTable, TableDescription, connectTable } from "./define-table"
import type { MigrationDefinition } from "./types/migration"
import { migrate } from "./utils/migrate"

const logger = createLogger("das:sqlite:database")

export interface DatabaseOptions {
  /**
   * Path to the sqlite database file.
   */
  path: string

  /**
   * Signed 32-bit integer identifying the type of SQLite database.
   *
   * This should be unique per type of database so that errors arising from opening an SQLite database that isn't meant for the current application can be caught early.
   */
  applicationId: number

  /**
   * A set of migrations to set up the database schema.
   */
  migrations: MigrationDefinition[]

  /**
   * A set of table definitions which provide a type-safe interface to the database.
   */
  tables?: Record<string, TableDescription> | undefined
}

export interface InferDatabaseInstance<TOptions extends DatabaseOptions> {
  tables: InferTableAccessors<TOptions>
  raw: Database.Database
}

export type InferTableAccessors<TOptions extends DatabaseOptions> =
  TOptions["tables"] extends Record<string, TableDescription>
    ? {
        [K in keyof TOptions["tables"]]: ConnectedTable<TOptions["tables"][K]>
      }
    : undefined

export const createDatabase = <TOptions extends DatabaseOptions>(
  databaseOptions: TOptions
): InferDatabaseInstance<TOptions> => {
  const database = new Database(databaseOptions.path)

  // SQLite INTEGERs are 64-bit signed integers, so we would like to get them as bigints from the database.
  database.defaultSafeIntegers(true)

  // If this is a brand-new, empty database, initialize it by setting the application ID
  const applicationIdBigint = BigInt(databaseOptions.applicationId)
  if (
    database.pragma("application_id", { simple: true }) === 0n &&
    database.pragma("schema_version", { simple: true }) === 0n
  ) {
    logger.info("initializing a new sqlite database")
    database.pragma(`application_id = ${applicationIdBigint}`)
  }

  // Database must have the correct application ID or we can't continue
  if (
    database.pragma("application_id", { simple: true }) !== applicationIdBigint
  ) {
    throw new Error("Database file is not a valid Dassie node database")
  }

  migrate(database, databaseOptions.migrations)

  return {
    tables: (databaseOptions.tables
      ? Object.fromEntries(
          Object.entries(databaseOptions.tables).map(([tableName, table]) => [
            tableName,
            connectTable(table, database),
          ])
        )
      : undefined) as InferTableAccessors<TOptions>,
    raw: database,
  }
}
