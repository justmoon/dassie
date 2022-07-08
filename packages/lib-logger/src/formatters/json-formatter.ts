import type { Formatter } from "../types/formatter"
import type { LogLine, SerializableLogLine } from "../types/log-line"
import type { LogErrorOptions } from "../types/log-options"
import isError from "../utils/is-error"

const serializeError = (error: unknown) => {
  if (isError(error)) {
    return error.stack
  }

  return String(error)
}

export interface JsonFormatterOptions {
  outputFunction?: (line: SerializableLogLine) => void
}

export const createJsonFormatter = ({
  outputFunction = (value) => console.log(JSON.stringify(value)),
}: JsonFormatterOptions = {}): Formatter => {
  const log = (
    { date, error, ...line }: LogLine,
    options: LogErrorOptions
  ): void => {
    if (options.ignoreInProduction && process.env["NODE_ENV"] === "production")
      return

    const serializedLine: SerializableLogLine = {
      date: date.toISOString(),
      ...(error ? { error: serializeError(error) } : {}),
      ...line,
    }
    outputFunction(serializedLine)
  }

  return log
}
