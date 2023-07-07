import { Reactor, ReadonlySignal, createSignal } from "@dassie/lib-reactive"

import type { VALID_REALMS } from "../constants/general"
import { databasePlain } from "../database/open-database"

export const ConfigLevel = {
  Null: 0,
  WebUi: 1,
  NodeIdentity: 2,
} as const

export type ConfigLevel = (typeof ConfigLevel)[keyof typeof ConfigLevel]

export type Config =
  | NullLevelConfig
  | WebUiLevelConfig
  | NodeIdentityLevelConfig

export interface NullLevelConfig {
  readonly hasWebUi: false
  readonly hasNodeIdentity: false

  readonly hostname: string
  readonly port: number
  readonly alias: string
  readonly url: string

  readonly exchangeRateUrl: string
  readonly internalAmountPrecision: number
}

export interface WebUiLevelConfig extends Omit<NullLevelConfig, "hasWebUi"> {
  readonly hasWebUi: true
  readonly realm: (typeof VALID_REALMS)[number]
  readonly tlsWebCert: string
  readonly tlsWebKey: string
}

export interface NodeIdentityLevelConfig
  extends Omit<WebUiLevelConfig, "hasNodeIdentity"> {
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
  const configPort = database.scalars.get("config.port") ?? 8443
  const configAlias = database.scalars.get("config.alias") ?? "anonymous"
  const configTlsWebCert = database.scalars.get("config.tls_web_cert")
  const configTlsWebKey = database.scalars.get("config.tls_web_key")
  const configTlsDassieCert = database.scalars.get("config.tls_dassie_cert")
  const configTlsDassieKey = database.scalars.get("config.tls_dassie_key")
  const configExchangeRateUrl = database.scalars.get("config.exchange_rate_url")
  const configInternalAmountPrecision = database.scalars.get(
    "config.internal_amount_precision"
  )

  let config: Config = {
    hasWebUi: false,
    hasNodeIdentity: false,
    hostname: configHostname,
    port: configPort,
    alias: configAlias,
    url: `https://${configHostname}${
      configPort === 443 ? "" : `:${configPort}`
    }`,
    exchangeRateUrl:
      configExchangeRateUrl ?? "https://api.coinbase.com/v2/exchange-rates",
    internalAmountPrecision: configInternalAmountPrecision ?? 12,
  }

  if (!configRealm || !configTlsWebCert || !configTlsWebKey) {
    return createSignal(config)
  }

  config = {
    ...config,
    hasWebUi: true,
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
