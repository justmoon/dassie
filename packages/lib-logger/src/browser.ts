import { createEnableChecker } from "./enabled"
import { createJsonFormatter } from "./formatters/json-formatter"
import { createLoggerFactory } from "./logger"

export type { Formatter } from "./types/formatter"
export type { LogLine, SerializableLogLine } from "./types/log-line"
export type { LogLineOptions } from "./types/log-options"

export { createCliFormatter } from "./formatters/cli-formatter"
export { createJsonFormatter } from "./formatters/json-formatter"
export type { JsonFormatterOptions } from "./formatters/json-formatter"
export { createEnableChecker } from "./enabled"
export { createLoggerFactory } from "./logger"
export { selectBySeed } from "./utils/select-by-seed"

export type { Logger } from "./logger"

/**
 * Factory for Browser loggers.
 *
 * @beta
 */
export const createLogger = createLoggerFactory({
  enableChecker: createEnableChecker(""),
  formatter: createJsonFormatter(),
})
