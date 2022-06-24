import type LogErrorOptions from "./log-error-options"

/**
 * Formatters process raw log messages for a specific channel such as a terminal or browser console.
 *
 * @beta
 */
export interface Formatter {
  clear(): void
  log(level: string, message: string, data?: Record<string, unknown>): void
  logError(error: unknown, options: LogErrorOptions): void
}

/**
 * @beta
 */
export type FormatterConstructor = new (component: string) => Formatter
