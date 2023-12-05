import { mkdirSync } from "node:fs"
import { resolve } from "node:path"

import { type Reactor, createComputed } from "@dassie/lib-reactive"
import { createDatabase } from "@dassie/lib-sqlite"

import { EnvironmentConfig } from "../config/environment-config"
import { DASSIE_DATABASE_SCHEMA } from "./schema"

export const BetterSqliteNativeBindingSignal = (reactor: Reactor) =>
  createComputed(reactor, () => {
    const { rootPath } = reactor.use(EnvironmentConfig)

    return import.meta.env.PROD
      ? resolve(rootPath, "lib/better_sqlite3.node")
      : undefined
  })

export const Database = (reactor: Reactor) => {
  const { dataPath } = reactor.use(EnvironmentConfig)
  const nativeBinding = reactor.use(BetterSqliteNativeBindingSignal).read()

  mkdirSync(dataPath, { recursive: true })

  const database = createDatabase({
    path: `${dataPath}/dassie.sqlite3`,
    schema: DASSIE_DATABASE_SCHEMA,
    nativeBinding,
    checkSchema: true,
  })

  reactor.onCleanup(() => {
    database.raw.close()
  })

  return database
}
