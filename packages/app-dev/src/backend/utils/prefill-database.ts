import { unlink } from "node:fs/promises"

import { DASSIE_DATABASE_SCHEMA } from "@dassie/app-node/src/backend/database/open-database"
import { createDatabase } from "@dassie/lib-sqlite"
import { isErrorWithCode } from "@dassie/lib-type-utils"

export interface PrefillDatabaseParameters {
  dataPath: string
}

export const prefillDatabase = async ({
  dataPath,
}: PrefillDatabaseParameters) => {
  const databasePath = `${dataPath}/dassie.sqlite3`

  // Delete the database if it already exists.
  try {
    await unlink(databasePath)
  } catch (error) {
    if (!isErrorWithCode(error, "ENOENT")) {
      throw error
    }
  }

  const database = createDatabase({
    path: databasePath,
    schema: DASSIE_DATABASE_SCHEMA,
  })

  database.scalars.set("config.realm", "test")

  database.raw.close()
}
