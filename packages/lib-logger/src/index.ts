import { createEnableChecker } from "./enabled"
import CliFormatter from "./formatters/cli-formatter"
import JsonFormatter from "./formatters/json-formatter"
import { createLoggerFactory } from "./logger"

export type { Formatter } from "./types/formatter"
export type { LogLine, SerializableLogLine } from "./types/log-line"
export type { LogLineOptions, LogErrorOptions } from "./types/log-options"

export { default as CliFormatter } from "./formatters/cli-formatter"
export { default as JsonFormatter } from "./formatters/json-formatter"
export type { JsonFormatterOptions } from "./formatters/json-formatter"
export { createEnableChecker } from "./enabled"
export { createLoggerFactory } from "./logger"
export { selectBySeed } from "./utils/select-by-seed"

export type { Logger } from "./logger"

/**
 * Factory for Node.js CLI loggers.
 *
 * @beta
 */
export const createLogger = createLoggerFactory({
  enableChecker: createEnableChecker(process.env["DEBUG"] ?? ""),
  formatter:
    process.env["XEN_LOG_FORMATTER"] === "json"
      ? new JsonFormatter()
      : new CliFormatter(),
})
