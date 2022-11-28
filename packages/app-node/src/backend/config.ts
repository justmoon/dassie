import envPaths from "env-paths"
import { ZodTypeAny, z } from "zod"

import { readFileSync } from "node:fs"

import { createLogger } from "@dassie/lib-logger"
import { createSignal } from "@dassie/lib-reactive"
import { isErrorWithCode } from "@dassie/lib-type-utils"

import { APP_NAME, VALID_REALMS } from "./constants/general"
import {
  SubnetConfig,
  subnetConfigSchema,
} from "./subnets/schemas/subnet-config"

const logger = createLogger("das:node:config")

export type RealmType = typeof VALID_REALMS[number]

export interface Config {
  nodeId: string
  realm: RealmType
  ilpAllocationScheme: "test" | "g"
  host: string
  port: number
  url: string
  dataPath: string
  tlsWebCert: string
  tlsWebKey: string
  tlsDassieCert: string
  tlsDassieKey: string
  beacons: { url: string }[]
  initialSubnets: SubnetConfig
}

export type InputConfig = z.infer<typeof inputConfigSchema>
export const inputConfigSchema = z.object({
  nodeId: z.string().optional(),
  realm: z.enum(VALID_REALMS).optional(),
  host: z.string().optional(),
  port: z.union([z.string(), z.number()]).optional(),
  url: z.string().optional(),
  dataPath: z.string().optional(),
  tlsWebCert: z.string().optional(),
  tlsWebCertFile: z.string().optional(),
  tlsWebKey: z.string().optional(),
  tlsWebKeyFile: z.string().optional(),
  tlsDassieCert: z.string().optional(),
  tlsDassieCertFile: z.string().optional(),
  tlsDassieKey: z.string().optional(),
  tlsDassieKeyFile: z.string().optional(),
  initialSubnets: z.string().optional(),
  beacons: z.string().optional(),
})

export const processFileOption = (
  name: string,
  value?: string,
  filePath?: string,
  defaultValue?: string
): string => {
  if (value) {
    return value
  } else if (filePath) {
    return readFileSync(filePath, "utf8")
  } else if (defaultValue) {
    return defaultValue
  } else {
    throw new Error(`Required option ${name}/${name}File is missing`)
  }
}

const parseJsonConfig = <T extends ZodTypeAny>(
  schema: T,
  value: string | undefined
): z.infer<T> | undefined => {
  try {
    // eslint-disable-next-line @typescript-eslint/no-unsafe-return
    return value ? (schema.parse(JSON.parse(value)) as z.infer<T>) : undefined
  } catch (error: unknown) {
    logger.warn("failed to parse JSON config", { error })
    return undefined
  }
}

export function fromPartialConfig(partialConfig: InputConfig): Config {
  const paths = envPaths(APP_NAME)

  const nodeId = partialConfig.nodeId ?? "anonymous"
  const realm = partialConfig.realm ?? (import.meta.env.DEV ? "test" : "live")
  const ilpAllocationScheme = realm === "test" ? "test" : "g"
  const host = partialConfig.host ?? "localhost"
  const port = partialConfig.port ? Number(partialConfig.port) : 8443

  return {
    nodeId,
    realm,
    ilpAllocationScheme,
    host,
    port,
    url:
      partialConfig.url ?? `https://${host}${port === 443 ? "" : `:${port}`}`,
    dataPath: partialConfig.dataPath ?? paths.data,
    tlsWebCert: processFileOption(
      "tlsWebCert",
      partialConfig.tlsWebCert,
      partialConfig.tlsWebCertFile
    ),
    tlsWebKey: processFileOption(
      "tlsWebKey",
      partialConfig.tlsWebKey,
      partialConfig.tlsWebKeyFile
    ),
    tlsDassieCert: processFileOption(
      "tlsDassieCert",
      partialConfig.tlsDassieCert,
      partialConfig.tlsDassieCertFile
    ),
    tlsDassieKey: processFileOption(
      "tlsDassieKey",
      partialConfig.tlsDassieKey,
      partialConfig.tlsDassieKeyFile
    ),
    initialSubnets: partialConfig.initialSubnets
      ? parseJsonConfig(subnetConfigSchema, partialConfig.initialSubnets) ?? []
      : [],
    beacons: (partialConfig.beacons ?? "")
      .split(";")
      .map((url) => ({ url }))
      .filter(Boolean),
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

export const configSignal = () => createSignal<Config>(fromEnvironment())
