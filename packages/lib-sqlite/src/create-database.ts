import Database from "better-sqlite3"

import { createLogger } from "@dassie/lib-logger"

import { checkSchema } from "./internal/check-schema"
import { type ConnectedTable, connectTable } from "./internal/connect-table"
import {
  type ScalarDescription,
  type ScalarStore,
  createScalarStore,
  scalarTable,
} from "./internal/scalar-store"
import type { MigrationDefinition } from "./types/migration"
import type { AnyTableDescription } from "./types/table"
import { migrate } from "./utils/migrate"

const logger = createLogger("das:sqlite:database")

export interface DatabaseSchema {
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
  tables?: Record<string, AnyTableDescription> | undefined

  /**
   * A set of scalar definitions which effectively provide a type-safe key-value store.
   */
  scalars?: Record<string, ScalarDescription> | undefined
}

export interface DatabaseOptions {
  /**
   * Path to the sqlite database file.
   */
  path: string

  /**
   * Metadata about the contents of the database.
   */
  schema: DatabaseSchema

  /**
   * Path to the better_sqlite3.node module.
   */
  nativeBinding?: string | undefined

  /**
   * Whether to check that the database schema matches the schema defined in the code.
   */
  checkSchema?: boolean | undefined
}

export interface DatabaseGenerics {
  schema: DatabaseSchema
}

export interface InferDatabaseInstance<T extends DatabaseGenerics> {
  tables: InferTableAccessors<T>
  scalars: InferScalarAccessors<T>
  raw: Database.Database
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type AnyDatabase = InferDatabaseInstance<any>

export type InferTableAccessors<TOptions extends DatabaseGenerics> =
  TOptions["schema"]["tables"] extends Record<string, AnyTableDescription>
    ? {
        [K in keyof TOptions["schema"]["tables"]]: ConnectedTable<
          TOptions["schema"]["tables"][K]
        >
      }
    : undefined

export type InferScalarAccessors<TOptions extends DatabaseGenerics> =
  TOptions["schema"]["scalars"] extends Record<string, ScalarDescription>
    ? ScalarStore<TOptions["schema"]["scalars"]>
    : undefined

export const createDatabase = <TOptions extends DatabaseOptions>(
  databaseOptions: TOptions
): InferDatabaseInstance<TOptions> => {
  const database = new Database(databaseOptions.path, {
    nativeBinding: databaseOptions.nativeBinding,
  })

  // SQLite INTEGERs are 64-bit signed integers, so we would like to get them as bigints from the database.
  database.defaultSafeIntegers(true)

  // If this is a brand-new, empty database, initialize it by setting the application ID
  const applicationIdBigint = BigInt(databaseOptions.schema.applicationId)
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
    throw new Error(
      "Database file has the wrong application ID - please make sure that the database file is not being shared between multiple applications"
    )
  }

  migrate(database, databaseOptions.schema.migrations)

  if (databaseOptions.checkSchema) {
    checkSchema(database, {
      scalarTable,
    })
    checkSchema(database, {
      ...databaseOptions.schema.tables,
    })
  }

  return {
    tables: (databaseOptions.schema.tables
      ? Object.fromEntries(
          Object.entries(databaseOptions.schema.tables).map(
            ([tableName, table]) => [tableName, connectTable(table, database)]
          )
        )
      : undefined) as InferTableAccessors<TOptions>,
    scalars: (databaseOptions.schema.scalars
      ? createScalarStore(
          databaseOptions.schema.scalars,
          connectTable(scalarTable, database)
        )
      : undefined) as InferScalarAccessors<TOptions>,
    raw: database,
  }
}
