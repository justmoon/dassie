import type { LogLevel } from "@dassie/lib-logger"
import { type Reactor, createSignal } from "@dassie/lib-reactive"

import { EnvironmentConfig } from "../../config/environment-config"

export type LogLevelOption = LogLevel | "none"
export const StdoutLogLevel = (reactor: Reactor) => {
  const { logLevel } = reactor.use(EnvironmentConfig)
  return createSignal<LogLevelOption>(logLevel)
}
