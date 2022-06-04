import Debug from "debug"
import envPaths from "env-paths"

import { readFile } from "node:fs/promises"

import { APP_NAME } from "./constants/general"

const debug = Debug(`${APP_NAME}:config`)

export interface Config {
  host: string
  port: number
  dataPath: string
  tlsWebCert: string
  tlsWebKey: string
  tlsXenCert: string
  tlsXenKey: string
}

export interface InputConfig {
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
}

export const processFileOption = async (
  name: string,
  value?: string,
  filePath?: string,
  defaultValue?: string
): Promise<string> => {
  if (value) {
    return value
  } else if (filePath) {
    return await readFile(filePath, "utf8")
  } else if (defaultValue) {
    return defaultValue
  } else {
    throw new Error(`Required option ${name}/${name}_FILE is missing`)
  }
}

export async function fromPartialConfig(partialConfig: InputConfig) {
  const paths = envPaths(APP_NAME, { suffix: "" })

  return {
    host: partialConfig.host ?? "localhost",
    port: partialConfig.port ? Number(partialConfig.port) : 8443,
    dataPath: partialConfig.dataPath ?? paths.data,
    tlsWebCert: await processFileOption(
      "TLS_WEB_CERT",
      partialConfig.tlsWebCert,
      partialConfig.tlsWebCertFile
    ),
    tlsWebKey: await processFileOption(
      "TLS_WEB_KEY",
      partialConfig.tlsWebKey,
      partialConfig.tlsWebKeyFile
    ),
    tlsXenCert: await processFileOption(
      "TLS_XEN_CERT",
      partialConfig.tlsXenCert,
      partialConfig.tlsXenCertFile
    ),
    tlsXenKey: await processFileOption(
      "TLS_XEN_KEY",
      partialConfig.tlsXenKey,
      partialConfig.tlsXenKeyFile
    ),
  }
}

export async function fromEnvironment() {
  const paths = envPaths(APP_NAME, { suffix: "" })
  const configPath = paths.config
  try {
    const config = await readFile(configPath, "utf8")
    return await fromPartialConfig(JSON.parse(config))
  } catch (error) {
    debug(`failed to read config file path=${configPath} err=${error}`)
  }

  return await fromPartialConfig({})
}
