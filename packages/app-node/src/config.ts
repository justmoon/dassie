import envPaths from "env-paths"
import { z } from "zod"

import { readFileSync } from "node:fs"

import { createLogger } from "@dassie/lib-logger"
import { createStore } from "@dassie/lib-reactive"
import { isErrorWithCode } from "@dassie/lib-type-utils"

import { APP_NAME } from "./constants/general"

const logger = createLogger("das:node:config")

export interface Config {
  nodeId: string
  subnetId: string
  ilpAddress: string
  host: string
  port: number
  dataPath: string
  tlsWebCert: string
  tlsWebKey: string
  tlsDassieCert: string
  tlsDassieKey: string
  initialPeers: { nodeId: string; url: string }[]
}

export type InputConfig = z.infer<typeof inputConfigSchema>

export const inputConfigSchema = z.object({
  nodeId: z.string().optional(),
  subnetId: z.string().optional(),
  host: z.string().optional(),
  port: z.union([z.string(), z.number()]).optional(),
  dataPath: z.string().optional(),
  tlsWebCert: z.string().optional(),
  tlsWebCertFile: z.string().optional(),
  tlsWebKey: z.string().optional(),
  tlsWebKeyFile: z.string().optional(),
  tlsDassieCert: z.string().optional(),
  tlsDassieCertFile: z.string().optional(),
  tlsDassieKey: z.string().optional(),
  tlsDassieKeyFile: z.string().optional(),
  initialPeers: z.string().optional(),
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

export function fromPartialConfig(partialConfig: InputConfig): Config {
  const paths = envPaths(APP_NAME)

  const nodeId = partialConfig.nodeId ?? "anonymous"
  const subnetId = partialConfig.subnetId ?? "none"
  return {
    nodeId,
    subnetId,
    ilpAddress: `g.das.${subnetId}.${nodeId}`,
    host: partialConfig.host ?? "localhost",
    port: partialConfig.port ? Number(partialConfig.port) : 8443,
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
    initialPeers: partialConfig.initialPeers
      ? partialConfig.initialPeers
          .split(";")
          .map((peer) => {
            const [nodeId, url] = peer.split("=")
            return { nodeId, url }
          })
          .filter(
            (peer): peer is { nodeId: string; url: string } =>
              peer.nodeId != null && peer.url != null
          )
      : [],
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

export const configStore = () => createStore<Config>(fromEnvironment())
