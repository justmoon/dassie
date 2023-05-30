import type { LogEvent } from "./log-event"

export type Formatter<T> = (value: T) => string

/**
 * Formatters process raw log messages for a specific channel such as a terminal or browser console.
 *
 * @beta
 */
export type LogEventFormatter = Formatter<LogEvent>
