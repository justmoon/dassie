import { command } from "cmd-ts"

import { createReactor } from "@dassie/lib-reactive"
import { createDatabase } from "@dassie/lib-sqlite"

import {
  DASSIE_DATABASE_SCHEMA,
  betterSqliteNativeBindingComputed,
} from "../../../database/open-database"

const verifyInstallCommand = command({
  name: "verify-install",
  description:
    "Verify that Dassie and its dependencies are correctly installed",
  args: {},
  handler() {
    const reactor = createReactor()
    const nativeBinding = reactor.use(betterSqliteNativeBindingComputed).read()

    // Create an in-memory database to verify that the sqlite native binding is working.
    console.info("Verifying that SQLite is working...")
    const database = createDatabase({
      path: ":memory:",
      schema: DASSIE_DATABASE_SCHEMA,
      nativeBinding,
    })

    // Verify that SQLite is working.
    database.raw.exec("SELECT 1 + 1;")

    console.info("Success! Dassie appears be to installed correctly.")
  },
})

export default verifyInstallCommand
