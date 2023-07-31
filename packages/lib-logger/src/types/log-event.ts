export interface LogMessage {
  type: "debug" | "info" | "warn" | "error"
  namespace: string
  date: number
  message: string
  parameters: unknown[]
}

export interface LogClear {
  type: "clear"
  date: number
}

export type LogEvent = LogMessage | LogClear
