import type { LogLevel } from "./log-level"

export interface LogMessage {
  type: LogLevel
  namespace: string
  date: number
  message: string
  parameters: unknown[]
  caller: string | undefined
}

export interface LogClear {
  type: "clear"
  date: number
}

export type LogEvent = LogMessage | LogClear
