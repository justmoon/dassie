import type { LogEventFormatter } from "../types/formatter"
import type { LogClear, LogMessage } from "../types/log-event"

export interface SerializableLogMessage {
  type: LogMessage["type"]
  date: string
  message: string
  parameters: unknown[]
}

export interface SerializableLogClear {
  type: LogClear["type"]
  date: string
}

export type SerializableLogEvent = SerializableLogMessage | SerializableLogClear

export interface JsonFormatterOptions {
  serializationFunction?: (line: SerializableLogEvent) => string
  outputFunction?: (line: string) => void
}

export const createJsonFormatter = ({
  serializationFunction = JSON.stringify,
}: JsonFormatterOptions = {}): LogEventFormatter => {
  const log: LogEventFormatter = (line) => {
    const serializedLine: SerializableLogEvent =
      line.type === "clear"
        ? {
            type: "clear",
            date: line.date.toISOString(),
          }
        : {
            ...line,
            date: line.date.toISOString(),
          }
    return serializationFunction(serializedLine)
  }

  return log
}
