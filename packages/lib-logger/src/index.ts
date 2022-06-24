import CliFormatter from "./formatters/cli-formatter"
import { _createLoggerFactory } from "./logger"

export type { default as LogErrorOptions } from "./types/log-error-options"

export type { Formatter, FormatterConstructor } from "./types/formatter"

export { createEnableChecker } from "./enabled"

export type { Logger } from "./logger"

/**
 * Factory for Node.js CLI loggers.
 *
 * @beta
 */
export const createLogger = _createLoggerFactory(
  process.env["DEBUG"] ?? "",
  CliFormatter
)
