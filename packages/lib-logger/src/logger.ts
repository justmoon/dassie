import CliFormatter from "./cli-formatter"
import createEnableChecker from "./enabled"

export let enableChecker = createEnableChecker(import.meta.env["DEBUG"])

export class Logger {
  readonly enabled: boolean
  readonly formatter: CliFormatter

  constructor(readonly component: string) {
    this.enabled = enableChecker(component)
    this.formatter = new CliFormatter(component)
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
}

export default function createLogger(component: string) {
  return new Logger(component)
}
