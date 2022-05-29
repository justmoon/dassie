import envPaths from "env-paths"
import { APP_NAME } from "../constants/general"
import { readFile } from "node:fs/promises"
import Debug from "debug"
const debug = Debug(`${APP_NAME}:config`)

interface ConfigSchema {
  dataPath: string
}

class Config {
  constructor(readonly data: ConfigSchema) {}

  static applyDefaults(partialConfig: Partial<ConfigSchema>): ConfigSchema {
    const paths = envPaths(APP_NAME, { suffix: "" })
    return {
      dataPath: partialConfig.dataPath ?? paths.data,
    }
  }

  static async fromEnvironment() {
    const paths = envPaths(APP_NAME, { suffix: "" })
    const configPath = paths.config
    try {
      const config = await readFile(configPath, "utf8")
      return new Config(Config.applyDefaults(JSON.parse(config)))
    } catch (error) {
      debug(`failed to read config file path=${configPath} err=${error}`)
    }

    return new Config(Config.applyDefaults({}))
  }
}

export default Config
