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
}

export interface WebUiLevelConfig extends Omit<NullLevelConfig, "hasWebUi"> {
  readonly hasWebUi: true
  readonly realm: ReadonlySignal<(typeof VALID_REALMS)[number]>
  readonly tlsWebCert: ReadonlySignal<string>
  readonly tlsWebKey: ReadonlySignal<string>
}

export interface NodeIdentityLevelConfig
  extends Omit<WebUiLevelConfig, "hasNodeIdentity"> {
  readonly hasNodeIdentity: true
  readonly tlsDassieCert: ReadonlySignal<string>
  readonly tlsDassieKey: ReadonlySignal<string>
}

export const databaseConfigPlain = (reactor: Reactor): Config => {
  const database = reactor.use(databasePlain)

  let config: Config = { hasWebUi: false, hasNodeIdentity: false }

  const configRealm = database.scalars.get("config.realm")
  const configTlsWebCert = database.scalars.get("config.tls_web_cert")
  const configTlsWebKey = database.scalars.get("config.tls_web_key")

  if (!configRealm || !configTlsWebCert || !configTlsWebKey) {
    return config
  }

  config = {
    ...config,
    hasWebUi: true,
    realm: createSignal(configRealm),
    tlsWebCert: createSignal(configTlsWebCert),
    tlsWebKey: createSignal(configTlsWebKey),
  }

  const configTlsDassieCert = database.scalars.get("config.tls_dassie_cert")
  const configTlsDassieKey = database.scalars.get("config.tls_dassie_key")

  if (!configTlsDassieCert || !configTlsDassieKey) {
    return config
  }

  config = {
    ...config,
    hasNodeIdentity: true,
    tlsDassieCert: createSignal(configTlsDassieCert),
    tlsDassieKey: createSignal(configTlsDassieKey),
  }

  return config
}
