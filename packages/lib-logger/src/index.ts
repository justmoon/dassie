import createEnableChecker from "./enabled"
import CliFormatter from "./formatters/cli-formatter"
import createLoggerFactory from "./logger"
import type { Logger } from "./logger"
import type { Formatter, FormatterConstructor } from "./types/formatter"

const createLogger = createLoggerFactory(
  process.env["DEBUG"] ?? "",
  CliFormatter
)

export {
  createLogger,
  createLoggerFactory,
  createEnableChecker,
  Logger,
  Formatter,
  FormatterConstructor,
}
