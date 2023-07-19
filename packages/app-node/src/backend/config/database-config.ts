import { Reactor, ReadonlySignal, createSignal } from "@dassie/lib-reactive"

import type { VALID_REALMS } from "../constants/general"
import { databasePlain } from "../database/open-database"

export const ConfigLevel = {
  Null: 0,
  WebUi: 1,
  NodeIdentity: 2,
} as const

export type ConfigLevel = (typeof ConfigLevel)[keyof typeof ConfigLevel]

export type Config = BaseLevelConfig | TlsLevelConfig | NodeIdentityLevelConfig

export interface BaseLevelConfig {
  readonly hasTls: false
  readonly hasNodeIdentity: false

  readonly hostname: string
  readonly httpPort: number
  readonly httpsPort: number
  readonly alias: string
  readonly url: string
  readonly enableHttpServer: boolean

  readonly exchangeRateUrl: string
  readonly internalAmountPrecision: number
}

export interface TlsLevelConfig extends Omit<BaseLevelConfig, "hasTls"> {
  readonly hasTls: true
  readonly realm: (typeof VALID_REALMS)[number]
  readonly tlsWebCert: string
  readonly tlsWebKey: string
}

export interface NodeIdentityLevelConfig
  extends Omit<TlsLevelConfig, "hasNodeIdentity"> {
  readonly hasNodeIdentity: true
  readonly tlsDassieCert: string
  readonly tlsDassieKey: string
}

export const databaseConfigSignal = (
  reactor: Reactor
): ReadonlySignal<Config> => {
  const database = reactor.use(databasePlain)

  const configRealm = database.scalars.get("config.realm")
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

  let config: Config = {
    hasTls: false,
    hasNodeIdentity: false,
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
  }

  if (!configRealm || !configTlsWebCert || !configTlsWebKey) {
    return createSignal(config)
  }

  config = {
    ...config,
    hasTls: true,
    realm: configRealm,
    tlsWebCert: configTlsWebCert,
    tlsWebKey: configTlsWebKey,
  }

  if (!configTlsDassieCert || !configTlsDassieKey) {
    return createSignal(config)
  }

  config = {
    ...config,
    hasNodeIdentity: true,
    tlsDassieCert: configTlsDassieCert,
    tlsDassieKey: configTlsDassieKey,
  }

  return createSignal(config)
}
