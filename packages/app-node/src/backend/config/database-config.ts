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
  const configRealm = database.scalars.configRealm.get() ?? "test"
  const configHostname = database.scalars.configHostname.get() ?? "localhost"
  const configHttpPort = database.scalars.configHttpPort.get() ?? 80
  const configHttpsPort = database.scalars.configHttpsPort.get() ?? 443
  const configAlias = database.scalars.configAlias.get() ?? "anonymous"
  const configEnableHttpServer =
    database.scalars.configEnableHttpServer.get() ?? true
  const configTlsWebCert = database.scalars.configTlsWebCert.get()
  const configTlsWebKey = database.scalars.configTlsWebKey.get()
  const configTlsDassieCert = database.scalars.configTlsDassieCert.get()
  const configTlsDassieKey = database.scalars.configTlsDassieKey.get()
  const configExchangeRateUrl = database.scalars.configExchangeRateUrl.get()
  const configInternalAmountPrecision =
    database.scalars.configInternalAmountPrecision.get()

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
        database.scalars.configRealm.set(realm)
        draft.realm = realm
      }),

    setTlsCertificates: (tlsCert: string, tlsKey: string) =>
      produce((draft) => {
        database.scalars.configTlsWebCert.set(tlsCert)
        database.scalars.configTlsWebKey.set(tlsKey)
        draft.tlsWebCert = tlsCert
        draft.tlsWebKey = tlsKey
      }),
  })
}
