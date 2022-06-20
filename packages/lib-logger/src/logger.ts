import createEnableChecker from "./enabled"
import type { Formatter, FormatterConstructor } from "./types/formatter"
import type LogErrorOptions from "./types/log-error-options"

export class Logger {
  readonly enabled: boolean
  readonly formatter: Formatter

  constructor(
    readonly component: string,
    Formatter: FormatterConstructor,
    enableChecker: (component: string) => boolean
  ) {
    this.enabled = enableChecker(component)
    this.formatter = new Formatter(component)
  }

  clear() {
    this.formatter.clear()
  }

  debug(message: string, data?: Record<string, unknown>) {
    if (this.enabled) {
      this.formatter.log("debug", message, data)
    }
  }

  info(message: string, data?: Record<string, unknown>) {
    this.formatter.log("info", message, data)
  }

  warn(message: string, data?: Record<string, unknown>) {
    this.formatter.log("warn", message, data)
  }

  error(message: string, data?: Record<string, unknown>) {
    this.formatter.log("error", message, data)
  }

  logError(error: unknown, options: LogErrorOptions = {}) {
    this.formatter.logError(error, options)
  }
}

export default function createLoggerFactory(
  scope: string,
  Formatter: FormatterConstructor
) {
  let enableChecker = createEnableChecker(scope)
  const createLogger = (component: string) => {
    return new Logger(component, Formatter, enableChecker)
  }
  return createLogger
}
