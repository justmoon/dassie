import { InferScalarType } from "@dassie/lib-sqlite"

import type { CONFIG_DATABASE_SCALARS } from "../schemas/database-scalars"

export type ConfigDatabaseScalarsWithoutPrefix = {
  [K in keyof typeof CONFIG_DATABASE_SCALARS as K extends `config.${infer R}`
    ? R
    : never]: (typeof CONFIG_DATABASE_SCALARS)[K]
}

export type InferTypescriptTypeFromConfigKey<
  TKey extends keyof ConfigDatabaseScalarsWithoutPrefix
> = InferScalarType<ConfigDatabaseScalarsWithoutPrefix[TKey]>
