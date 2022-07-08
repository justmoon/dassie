import envPaths from "env-paths"

import { readFileSync } from "node:fs"

import { createLogger } from "@xen-ilp/lib-logger"
import { createStore } from "@xen-ilp/lib-reactive"
import { isErrorWithCode } from "@xen-ilp/lib-type-utils"

import { APP_NAME } from "./constants/general"

const logger = createLogger("xen:node:config")

export interface Config {
  nodeId: string
  host: string
  port: number
  dataPath: string
  tlsWebCert: string
  tlsWebKey: string
  tlsXenCert: string
  tlsXenKey: string
  initialPeers: { nodeId: string; url: string }[]
}

export interface InputConfig {
  nodeId?: string
  host?: string
  port?: string | number
  dataPath?: string
  tlsWebCert?: string
  tlsWebCertFile?: string
  tlsWebKey?: string
  tlsWebKeyFile?: string
  tlsXenCert?: string
  tlsXenCertFile?: string
  tlsXenKey?: string
  tlsXenKeyFile?: string
  initialPeers?: string
}

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
    throw new Error(`Required option ${name}/${name}_FILE is missing`)
  }
}

export function fromPartialConfig(partialConfig: InputConfig): Config {
  const paths = envPaths(APP_NAME, { suffix: "" })

  return {
    nodeId: partialConfig.nodeId ?? "anonymous",
    host: partialConfig.host ?? "localhost",
    port: partialConfig.port ? Number(partialConfig.port) : 8443,
    dataPath: partialConfig.dataPath ?? paths.data,
    tlsWebCert: processFileOption(
      "TLS_WEB_CERT",
      partialConfig.tlsWebCert,
      partialConfig.tlsWebCertFile
    ),
    tlsWebKey: processFileOption(
      "TLS_WEB_KEY",
      partialConfig.tlsWebKey,
      partialConfig.tlsWebKeyFile
    ),
    tlsXenCert: processFileOption(
      "TLS_XEN_CERT",
      partialConfig.tlsXenCert,
      partialConfig.tlsXenCertFile
    ),
    tlsXenKey: processFileOption(
      "TLS_XEN_KEY",
      partialConfig.tlsXenKey,
      partialConfig.tlsXenKeyFile
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
  const configPath = process.env["XEN_CONFIG_FILE"]

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

  // TODO: Validate using something like zod
  const environmentConfig = JSON.parse(
    process.env["XEN_CONFIG"] ?? "{}"
  ) as InputConfig

  return fromPartialConfig({ ...fileConfig, ...environmentConfig })
}

export const configStore = createStore<Config>("config", fromEnvironment())
