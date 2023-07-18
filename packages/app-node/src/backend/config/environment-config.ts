import envPaths from "env-paths"
import { ZodTypeAny, z } from "zod"

import { createSignal } from "@dassie/lib-reactive"

import { APP_NAME, VALID_REALMS } from "../constants/general"
import { nodeIdSchema } from "./schemas/node-id"
import { EnvironmentVariables } from "./types/environment-variables"

export type RealmType = (typeof VALID_REALMS)[number]

export interface Config {
  rootPath: string
  dataPath: string
  cachePath: string
  temporaryPath: string
  ipcSocketPath: string
  bootstrapNodes: BootstrapNodesConfig
}

export const bootstrapNodesSchema = z.array(
  z.object({
    nodeId: nodeIdSchema,
    url: z.string(),
    alias: z.string(),
    nodePublicKey: z.string(),
  })
)

export type BootstrapNodesConfig = z.infer<typeof bootstrapNodesSchema>

function parseConfigOptionWithSchema<TSchema extends ZodTypeAny>(
  optionName: keyof EnvironmentVariables,
  schema: TSchema,
  defaultValue?: z.infer<TSchema>
): z.infer<TSchema> {
  const option = process.env[optionName]
  if (option === undefined && defaultValue !== undefined) {
    // eslint-disable-next-line @typescript-eslint/no-unsafe-return
    return defaultValue
  }

  if (option === undefined) {
    throw new Error(`Missing required environment variable ${optionName}`)
  }

  // eslint-disable-next-line @typescript-eslint/no-unsafe-return
  return schema.parse(JSON.parse(option)) as z.infer<TSchema>
}

export function fromEnvironment(): Config {
  const paths = envPaths(APP_NAME)
  const environment = process.env as EnvironmentVariables

  return {
    rootPath: environment.DASSIE_ROOT ?? process.cwd(),
    dataPath:
      environment.DASSIE_STATE_DIRECTORY ??
      environment.STATE_DIRECTORY ??
      paths.data,
    cachePath:
      environment.DASSIE_CACHE_DIRECTORY ??
      environment.CACHE_DIRECTORY ??
      paths.cache,
    temporaryPath: environment.DASSIE_TEMPORARY_DIRECTORY ?? paths.temp,
    ipcSocketPath: environment.DASSIE_IPC_SOCKET_PATH ?? "/run/dassie.sock",
    bootstrapNodes: parseConfigOptionWithSchema(
      "DASSIE_BOOTSTRAP_NODES",
      bootstrapNodesSchema,
      []
    ),
  }
}

export const environmentConfigSignal = () =>
  createSignal<Config>(fromEnvironment())
