import { produce } from "immer"

import { Reactor, createStore } from "@dassie/lib-reactive"

import type { VALID_REALMS } from "../constants/general"
import { databasePlain } from "../database/open-database"

export interface Config {
  readonly realm: (typeof VALID_REALMS)[number]

  readonly hostname: string
  readonly httpPort: number
  readonly httpsPort: number
  readonly alias: string
  readonly url: string
  readonly enableHttpServer: boolean

  readonly exchangeRateUrl: string
  readonly internalAmountPrecision: number

  readonly tlsWebCert: string | undefined
  readonly tlsWebKey: string | undefined

  readonly tlsDassieCert: string | undefined
  readonly tlsDassieKey: string | undefined
}

export const hasTls = (
  config: Config
): config is Config & { tlsWebCert: string; tlsWebKey: string } => {
  return config.tlsWebCert !== undefined && config.tlsWebKey !== undefined
}

export const hasNodeIdentity = (
  config: Config
): config is Config & { tlsDassieCert: string; tlsDassieKey: string } => {
  return config.tlsDassieCert !== undefined && config.tlsDassieKey !== undefined
}

const loadInitialConfig = (
  database: ReturnType<typeof databasePlain>
): Config => {
  const configRealm = database.scalars.get("config.realm") ?? "test"
  const configHostname = database.scalars.get("config.hostname") ?? "localhost"
  const configHttpPort = database.scalars.get("config.http_port") ?? 80
  const configHttpsPort = database.scalars.get("config.https_port") ?? 443
  const configAlias = database.scalars.get("config.alias") ?? "anonymous"
  const configEnableHttpServer =
    database.scalars.get("config.enable_http_server") ?? true
  const configTlsWebCert = database.scalars.get("config.tls_web_cert")
  const configTlsWebKey = database.scalars.get("config.tls_web_key")
  const configTlsDassieCert = database.scalars.get("config.tls_dassie_cert")
  const configTlsDassieKey = database.scalars.get("config.tls_dassie_key")
  const configExchangeRateUrl = database.scalars.get("config.exchange_rate_url")
  const configInternalAmountPrecision = database.scalars.get(
    "config.internal_amount_precision"
  )

  return {
    realm: configRealm,

    hostname: configHostname,
    httpPort: configHttpPort,
    httpsPort: configHttpsPort,
    alias: configAlias,
    url: `https://${configHostname}${
      configHttpsPort === 443 ? "" : `:${configHttpsPort}`
    }`,
    enableHttpServer: configEnableHttpServer,
    exchangeRateUrl:
      configExchangeRateUrl ?? "https://api.coinbase.com/v2/exchange-rates",
    internalAmountPrecision: configInternalAmountPrecision ?? 12,

    tlsWebCert: configTlsWebCert,
    tlsWebKey: configTlsWebKey,

    tlsDassieCert: configTlsDassieCert,
    tlsDassieKey: configTlsDassieKey,
  }
}

export const databaseConfigStore = (reactor: Reactor) => {
  const database = reactor.use(databasePlain)

  return createStore(loadInitialConfig(database), {
    setRealm: (realm: (typeof VALID_REALMS)[number]) =>
      produce((draft) => {
        database.scalars.set("config.realm", realm)
        draft.realm = realm
      }),

    setTlsCertificates: (tlsCert: string, tlsKey: string) =>
      produce((draft) => {
        database.scalars.set("config.tls_web_cert", tlsCert)
        database.scalars.set("config.tls_web_key", tlsKey)
        draft.tlsWebCert = tlsCert
        draft.tlsWebKey = tlsKey
      }),
  })
}
