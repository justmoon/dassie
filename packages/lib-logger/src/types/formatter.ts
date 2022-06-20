import type LogErrorOptions from "./log-error-options"

export interface Formatter {
  clear(): void
  log(level: string, message: string, data?: Record<string, unknown>): void
  logError(error: unknown, options: LogErrorOptions): void
}

export interface FormatterConstructor {
  new (component: string): Formatter
}
