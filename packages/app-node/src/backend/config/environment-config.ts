import envPaths from "env-paths"
import { ReadonlyDeep } from "type-fest"
import { z } from "zod"

import { readFileSync } from "node:fs"

import { createLogger } from "@dassie/lib-logger"
import { createSignal } from "@dassie/lib-reactive"
import { isErrorWithCode } from "@dassie/lib-type-utils"

import { APP_NAME, VALID_REALMS } from "../constants/general"
import { nodeIdSchema } from "./schemas/node-id"

const logger = createLogger("das:node:config")

export type RealmType = (typeof VALID_REALMS)[number]

export interface Config {
  rootPath: string
  dataPath: string
  cachePath: string
  ipcSocketPath: string
  bootstrapNodes: NonNullable<InputConfig["bootstrapNodes"]>
}

export type InputConfig = ReadonlyDeep<z.infer<typeof inputConfigSchema>>
export const inputConfigSchema = z.object({
  rootPath: z.string().optional(),
  dataPath: z.string().optional(),
  cachePath: z.string().optional(),
  ipcSocketPath: z.string().optional(),
  bootstrapNodes: z
    .array(
      z.object({
        nodeId: nodeIdSchema,
        url: z.string(),
        alias: z.string(),
        nodePublicKey: z.string(),
      })
    )
    .optional(),
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
    cachePath:
      partialConfig.cachePath ??
      process.env["DASSIE_CACHE_DIRECTORY"] ??
      process.env["CACHE_DIRECTORY"] ??
      paths.cache,
    ipcSocketPath: process.env["DASSIE_IPC_SOCKET_PATH"] ?? "/run/dassie.sock",
    bootstrapNodes: partialConfig.bootstrapNodes ?? [],
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
