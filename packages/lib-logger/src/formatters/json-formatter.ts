import { isObject } from "@xen-ilp/lib-type-utils"

import type { Formatter } from "../types/formatter"
import type { LogLine, SerializableLogLine } from "../types/log-line"
import type { LogErrorOptions } from "../types/log-options"

const serializeError = (error: unknown) => {
  if (
    isObject(error) &&
    "stack" in error &&
    "string" === typeof error["stack"]
  ) {
    return error["stack"]
  }

  return String(error)
}

export interface JsonFormatterOptions {
  outputFunction: (line: SerializableLogLine) => void
}

export default class JsonFormatter implements Formatter {
  constructor(
    readonly options: JsonFormatterOptions = {
      outputFunction: (value: SerializableLogLine) =>
        console.log(JSON.stringify(value)),
    }
  ) {}

  clear(): void {
    const serializedLine: SerializableLogLine = {
      component: "xen:logger",
      date: new Date().toISOString(),
      level: "info",
      message: "%%clear%%",
    }
    this.options.outputFunction(serializedLine)
  }

  log({ date, error, ...line }: LogLine, options: LogErrorOptions): void {
    if (options.ignoreInProduction && process.env["NODE_ENV"] === "production")
      return

    const serializedLine: SerializableLogLine = {
      date: date.toISOString(),
      ...(error ? { error: serializeError(error) } : {}),
      ...line,
    }
    this.options.outputFunction(serializedLine)
  }
}
