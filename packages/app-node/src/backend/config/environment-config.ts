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
  rootPath: string
  dataPath: string
  runtimePath: string
  initialSubnets: SubnetConfig
}

export type InputConfig = ReadonlyDeep<z.infer<typeof inputConfigSchema>>
export const inputConfigSchema = z.object({
  rootPath: z.string().optional(),
  dataPath: z.string().optional(),
  initialSubnets: subnetConfigSchema.optional(),
})

export function fromPartialConfig(partialConfig: InputConfig): Config {
  const paths = envPaths(APP_NAME)

  return {
    rootPath: partialConfig.rootPath ?? process.cwd(),
    dataPath:
      partialConfig.dataPath ??
      process.env["DASSIE_STATE_DIRECTORY"] ??
      process.env["STATE_DIRECTORY"] ??
      paths.data,
    runtimePath:
      process.env["DASSIE_RUNTIME_DIRECTORY"] ??
      process.env["RUNTIME_DIRECTORY"] ??
      "/run/dassie",
    initialSubnets: partialConfig.initialSubnets ?? [],
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

  return fromPartialConfig({
    rootPath: process.env["DASSIE_ROOT"],
    ...fileConfig,
    ...environmentConfig,
  })
}

export const environmentConfigSignal = () =>
  createSignal<Config>(fromEnvironment())
