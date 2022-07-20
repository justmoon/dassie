export interface LogLine {
  component: string
  date: Date
  level: "debug" | "info" | "warn" | "error" | "clear"
  message: string
  data?: Record<string, unknown>
}

export interface SerializableLogLine {
  component: string
  date: string
  level: "debug" | "info" | "warn" | "error" | "clear"
  message: string
  data?: Record<string, string>
}
