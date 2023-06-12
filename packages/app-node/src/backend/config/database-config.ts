import { Reactor, createSignal } from "@dassie/lib-reactive"

import { databasePlain } from "../database/open-database"

export const databaseConfigPlain = (reactor: Reactor) => {
  const database = reactor.use(databasePlain)

  const configRealm = database.scalars.get("config.realm")

  if (!configRealm) {
    throw new Error("Missing required configuration value: config.realm")
  }

  const configTlsWebCert = database.scalars.get("config.tls_web_cert")

  if (!configTlsWebCert) {
    throw new Error("Missing required configuration value: config.tls_web_cert")
  }

  const configTlsWebKey = database.scalars.get("config.tls_web_key")

  if (!configTlsWebKey) {
    throw new Error("Missing required configuration value: config.tls_web_key")
  }

  const configTlsDassieCert = database.scalars.get("config.tls_dassie_cert")

  if (!configTlsDassieCert) {
    throw new Error(
      "Missing required configuration value: config.tls_dassie_cert"
    )
  }

  const configTlsDassieKey = database.scalars.get("config.tls_dassie_key")

  if (!configTlsDassieKey) {
    throw new Error(
      "Missing required configuration value: config.tls_dassie_key"
    )
  }

  const config = {
    realm: createSignal(configRealm),
    tlsWebCert: createSignal(configTlsWebCert),
    tlsWebKey: createSignal(configTlsWebKey),
    tlsDassieCert: createSignal(configTlsDassieCert),
    tlsDassieKey: createSignal(configTlsDassieKey),
  }

  return {
    ...config,
  }
}
