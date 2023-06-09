import { Reactor, createSignal } from "@dassie/lib-reactive"

import { databasePlain } from "../database/open-database"

export const databaseConfigPlain = (reactor: Reactor) => {
  const database = reactor.use(databasePlain)

  const configRealm = database.scalars.get("config.realm")

  if (!configRealm) {
    throw new Error("Missing required configuration value: config.realm")
  }

  const config = {
    realm: createSignal(configRealm),
  }

  return {
    ...config,
  }
}
