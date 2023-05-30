export interface LogMessage {
  type: "debug" | "info" | "warn" | "error"
  date: Date
  message: string
  parameters: unknown[]
}

export interface LogClear {
  type: "clear"
  date: Date
}

export type LogEvent = LogMessage | LogClear
