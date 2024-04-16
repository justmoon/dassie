import type { LogLevel } from "../types/log-level"

const NUMERIC_LEVELS: Record<LogLevel, number> = {
  debug: 0,
  info: 1,
  warn: 2,
  error: 3,
}

export function compareLogLevel(a: LogLevel, b: LogLevel) {
  return NUMERIC_LEVELS[a] - NUMERIC_LEVELS[b]
}
