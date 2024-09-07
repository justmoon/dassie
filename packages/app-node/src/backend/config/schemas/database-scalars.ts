import { z } from "zod"

import { type NamespacedScalars, scalar } from "@dassie/lib-sqlite"

import { VALID_REALMS } from "../../constants/general"
import type { RealmType } from "../environment-config"

export const portSchema = z.coerce.number().int().min(1).max(65_535)

/**
 * This defines all the scalar values that are stored in the database which will
 * allow us to access them in a type-safe way.
 */
export const CONFIG_DATABASE_SCALARS = {
  configRealm: scalar()
    .name("config.realm")
    .type("TEXT")
    .deserialize((value) => z.enum(VALID_REALMS).parse(value))
    .serialize((value: RealmType) => value),
  configHostname: scalar().name("config.hostname").type("TEXT"),
  configHttpPort: scalar()
    .name("config.http_port")
    .type("INTEGER")
    .deserialize((value) => portSchema.parse(value))
    .serialize(BigInt),
  configHttpsPort: scalar()
    .name("config.https_port")
    .type("INTEGER")
    .deserialize((value) => portSchema.parse(value))
    .serialize(BigInt),
  configAlias: scalar().name("config.alias").type("TEXT"),
  configTlsWebCert: scalar().name("config.tls_web_cert").type("TEXT"),
  configTlsWebKey: scalar().name("config.tls_web_key").type("TEXT"),
  configDassieKey: scalar().name("config.dassie_key").type("TEXT"),
  configExchangeRateUrl: scalar().name("config.exchange_rate_url").type("TEXT"),
  configEnableHttpServer: scalar()
    .name("config.enable_http_server")
    .type("INTEGER")
    .deserialize((value) =>
      z.coerce.number().int().min(0).max(1).transform(Boolean).parse(value),
    )
    .serialize((value: boolean) => (value ? 1n : 0n)),
} as const satisfies NamespacedScalars<"config">
