import { readFile, unlink } from "node:fs/promises"

import { DASSIE_DATABASE_SCHEMA } from "@dassie/app-node/src/backend/database/open-database"
import { createDatabase } from "@dassie/lib-sqlite"
import { isErrorWithCode } from "@dassie/lib-type-utils"

import { NodeConfig } from "./generate-node-config"

export const prefillDatabase = async ({
  config: { dataPath },
  tlsDassieCertFile,
  tlsDassieKeyFile,
  tlsWebCertFile,
  tlsWebKeyFile,
}: NodeConfig) => {
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

  database.scalars.set(
    "config.tls_dassie_cert",
    await readFile(tlsDassieCertFile, "utf8")
  )
  database.scalars.set(
    "config.tls_dassie_key",
    await readFile(tlsDassieKeyFile, "utf8")
  )
  database.scalars.set(
    "config.tls_web_cert",
    await readFile(tlsWebCertFile, "utf8")
  )
  database.scalars.set(
    "config.tls_web_key",
    await readFile(tlsWebKeyFile, "utf8")
  )

  database.raw.close()
}
