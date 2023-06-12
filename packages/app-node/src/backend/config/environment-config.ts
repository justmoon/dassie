import envPaths from "env-paths"
import { ReadonlyDeep } from "type-fest"
import { z } from "zod"

import { readFileSync } from "node:fs"

import { createLogger } from "@dassie/lib-logger"
import { createSignal } from "@dassie/lib-reactive"
import { isErrorWithCode } from "@dassie/lib-type-utils"

import { APP_NAME, VALID_REALMS } from "../constants/general"
import {
  type SubnetConfig,
  subnetConfigSchema,
} from "../subnets/schemas/subnet-config"

const logger = createLogger("das:node:config")

export type RealmType = (typeof VALID_REALMS)[number]

export interface Config {
  host: string
  port: number
  url: string
  alias: string
  rootPath: string
  dataPath: string
  initialSubnets: SubnetConfig
  exchangeRateUrl: string
  internalAmountPrecision: number
}

export type InputConfig = ReadonlyDeep<z.infer<typeof inputConfigSchema>>
export const inputConfigSchema = z.object({
  host: z.string().optional(),
  port: z.union([z.string(), z.number()]).optional(),
  url: z.string().optional(),
  alias: z.string().optional(),
  rootPath: z.string().optional(),
  dataPath: z.string().optional(),
  initialSubnets: subnetConfigSchema.optional(),
  exchangeRateUrl: z.string().optional(),
  internalAmountPrecision: z.number().optional(),
})

export function fromPartialConfig(partialConfig: InputConfig): Config {
  const paths = envPaths(APP_NAME)

  const host = partialConfig.host ?? "localhost"
  const port = partialConfig.port ? Number(partialConfig.port) : 8443

  return {
    host,
    port,
    alias: partialConfig.alias ?? "anonymous",
    url:
      partialConfig.url ?? `https://${host}${port === 443 ? "" : `:${port}`}`,
    rootPath: partialConfig.rootPath ?? process.cwd(),
    dataPath: partialConfig.dataPath ?? paths.data,
    initialSubnets: partialConfig.initialSubnets ?? [],
    exchangeRateUrl:
      partialConfig.exchangeRateUrl ??
      "https://api.coinbase.com/v2/exchange-rates",
    internalAmountPrecision: partialConfig.internalAmountPrecision ?? 12,
  }
}

export function fromEnvironment() {
  const configPath = process.env["DASSIE_CONFIG_FILE"]

  let fileConfig = {}
  if (configPath) {
    try {
      // TODO: Validate using something like zod
      fileConfig = JSON.parse(readFileSync(configPath, "utf8")) as InputConfig
    } catch (error) {
      if (isErrorWithCode(error, "ENOENT")) {
        logger.debug("config file not found", { path: configPath })
      } else {
        logger.debug("failed to read config file", { path: configPath, error })
      }
    }
  }

  const environmentConfig = inputConfigSchema.parse(
    JSON.parse(process.env["DASSIE_CONFIG"] ?? "{}")
  )

  return fromPartialConfig({ ...fileConfig, ...environmentConfig })
}

export const environmentConfigSignal = () =>
  createSignal<Config>(fromEnvironment())
