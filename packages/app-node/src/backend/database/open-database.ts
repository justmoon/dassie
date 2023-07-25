import { mkdirSync } from "node:fs"
import { resolve } from "node:path"

import { type Reactor, createComputed } from "@dassie/lib-reactive"
import { createDatabase } from "@dassie/lib-sqlite"

import { environmentConfigSignal } from "../config/environment-config"
import { DASSIE_DATABASE_SCHEMA } from "./schema"

export const betterSqliteNativeBindingComputed = () =>
  createComputed((sig) => {
    const { rootPath } = sig.use(environmentConfigSignal).read()

    return import.meta.env.PROD
      ? resolve(rootPath, "lib/better_sqlite3.node")
      : undefined
  })

export const databasePlain = (reactor: Reactor) => {
  const { dataPath } = reactor.use(environmentConfigSignal).read()
  const nativeBinding = reactor.use(betterSqliteNativeBindingComputed).read()

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
