import { format as prettyFormat } from "pretty-format"

import type { Formatter } from "../types/formatter"
import type { LogLine, SerializableLogLine } from "../types/log-line"
import type { LogLineOptions } from "../types/log-options"
import { isError } from "../utils/is-error"
import { formatError } from "./cli-formatter"

const serializeData = (
  data: Record<string, unknown>,
  options: LogLineOptions
): Record<string, string> => {
  return Object.fromEntries(
    Object.entries(data).map(([key, value]) => {
      if (typeof value === "function") {
        value = value()
      }

      return [
        key,
        isError(value)
          ? formatError(value, options)
          : typeof value === "string"
          ? value
          : prettyFormat(value),
      ] as const
    })
  )
}

export interface JsonFormatterOptions {
  outputFunction?: (line: SerializableLogLine) => void
}

export const createJsonFormatter = ({
  outputFunction = (value) => console.log(JSON.stringify(value)),
}: JsonFormatterOptions = {}): Formatter => {
  const log = ({ date, ...line }: LogLine, options: LogLineOptions): void => {
    if (options.ignoreInProduction && process.env["NODE_ENV"] === "production")
      return

    const serializedLine: SerializableLogLine = {
      ...line,
      date: date.toISOString(),
      data: serializeData(line.data ?? {}, options),
    }
    outputFunction(serializedLine)
  }

  return log
}
