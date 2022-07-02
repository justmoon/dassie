import type { LogLine } from "./log-line"
import type { LogErrorOptions } from "./log-options"

/**
 * Formatters process raw log messages for a specific channel such as a terminal or browser console.
 *
 * @beta
 */
export interface Formatter {
  clear(): void
  log(line: LogLine, options: LogErrorOptions): void
}
