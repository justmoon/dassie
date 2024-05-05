import type { Database } from "better-sqlite3"

import { type Logger, createLogger } from "@dassie/lib-logger"

import type { MigrationDefinition } from "../types/migration"

export interface MigrationOptions {
  dangerouslyRedoLastMigration?: boolean | undefined
  logger?: Logger | undefined
}

export const migrate = (
  database: Database,
  migrations: MigrationDefinition[],
  options: MigrationOptions = {},
) => {
  const logger = options.logger ?? createLogger("das:sqlite:migrate")

  const currentUserVersion = Number(
    database.pragma("user_version", {
      simple: true,
    }) as number | bigint,
  )

  // Check integrity of migrations array
  let lastMigrationId = 0
  for (const [index, migration] of migrations.entries()) {
    if (migration.version !== ++lastMigrationId) {
      throw new Error(
        `Migration definitions are invalid. Migration at index ${index} was expected to have migration version ${lastMigrationId} but has version ${migration.version}.`,
      )
    }
  }

  if (currentUserVersion > lastMigrationId) {
    throw new Error(
      "Database is newer than the current version of the app, cannot continue",
    )
  }

  if (
    currentUserVersion === lastMigrationId &&
    options.dangerouslyRedoLastMigration
  ) {
    logger.debug?.("reapplying last migration", { lastMigrationId })
    // Re-apply last migration
    database.transaction(() => {
      migrations[currentUserVersion - 1]!.down(database)
      migrations[currentUserVersion - 1]!.up(database)
    })()
  }

  if (currentUserVersion < lastMigrationId) {
    logger.debug?.("applying migrations", {
      currentUserVersion,
      lastMigrationId,
    })

    // Migrate up
    for (
      let currentMigrationId = currentUserVersion + 1;
      currentMigrationId <= lastMigrationId;
      currentMigrationId++
    ) {
      const migration = migrations[currentMigrationId - 1]!
      database.transaction(() => {
        logger.debug?.("applying migration", { version: migration.version })

        migration.up(database)
        database.pragma(`user_version = ${migration.version}`)
      })()
    }
  }
}
