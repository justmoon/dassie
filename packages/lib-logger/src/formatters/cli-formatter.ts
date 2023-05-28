import chalk from "chalk"
import { format as prettyFormat } from "pretty-format"

import type { Formatter } from "../types/formatter"
import type { LogLine } from "../types/log-line"
import type { LogLineOptions } from "../types/log-options"
import { isError } from "../utils/is-error"
import { selectBySeed } from "../utils/select-by-seed"

export const COLORS = [
  chalk.red,
  chalk.green,
  chalk.yellow,
  chalk.blue,
  chalk.magenta,
  chalk.cyan,
] as const

export const formatFilePath = (filePath: string) => {
  let protocolPrefix = ""
  if (filePath.startsWith("file://")) {
    protocolPrefix = "file://"
    filePath = filePath.slice(7)
  }

  const match = filePath.match(/^(.*)\/packages\/([\da-z-]+)\/(.*)$/) as
    | [string, string, string, string]
    | null
  if (match) {
    return `${chalk.dim(`${protocolPrefix}${match[1]}packages/`)}${chalk.cyan(
      match[2]
    )}${chalk.dim("/")}${match[3]}`
  }

  return filePath
}

export const formatValue = (value: unknown, options: LogLineOptions) => {
  if (typeof value === "function") {
    value = value()
  }

  if (typeof value === "string") {
    return value
  } else if (isError(value)) {
    return formatError(value, options)
  }

  return prettyFormat(value)
}

const prefixCache = new Map<string, string>()

const getPrefix = (component: string) => {
  let prefix = prefixCache.get(component)
  if (!prefix) {
    prefix = selectBySeed(COLORS, component)(component) + " "
    prefixCache.set(component, prefix)
  }

  return prefix
}

export const formatError = (error: Error, options: LogLineOptions): string => {
  return error.stack
    ? formatStack(error.stack, options)
    : `${chalk.red(chalk.bold(`${error.name || "Error"}:`))} ${error.message}`
}

export const formatStack = (
  stack: string,
  options: LogLineOptions = {}
): string => {
  const lines = stack.split("\n")

  if (!lines[0]) return stack

  const [name, message] = lines[0]?.split(": ") ?? []

  let ignoreRest = false
  const formattedCallsites = lines
    .slice(1)
    .map((line) => {
      if (ignoreRest) return null
      {
        const match = line.match(/^ {4}at (.*) \((.*):(\d+):(\d+)\)$/) as
          | [string, string, string, string, string]
          | null
        if (match) {
          if (match[2].startsWith("node:")) {
            return chalk.dim(line)
          }
          if (options.skipAfter && match[1].includes(options.skipAfter)) {
            ignoreRest = true
          }
          const isNodeModules = match[2].includes("node_modules")
          const formattedFilePath = isNodeModules
            ? chalk.dim(match[2])
            : formatFilePath(match[2])

          return `    ${chalk.dim("at")} ${
            isNodeModules ? chalk.dim(match[1]) : match[1]
          } ${chalk.dim("(")}${formattedFilePath}${chalk.dim(
            `:${match[3]}:${match[4]})`
          )}`
        }
      }
      {
        const match = line.match(/^ {4}at (.*):(\d+):(\d+)$/) as
          | [string, string, string, string]
          | null
        if (match) {
          const formattedFilePath = formatFilePath(match[1])
          return `    ${chalk.dim("at")} ${formattedFilePath}${chalk.dim(
            `:${match[2]}:${match[3]}`
          )}`
        }
      }

      return chalk.dim(line)
    })
    .filter(Boolean)

  return `${chalk.red(chalk.bold(`${name ?? "Error"}:`))} ${chalk.red(
    message ?? "no message"
  )}${formattedCallsites.length > 0 ? "\n" : ""}${formattedCallsites.join(
    "\n"
  )}`
}

const log = (line: LogLine, options: LogLineOptions) => {
  if (line.level === "clear") {
    console.clear()
    return
  }

  const levelInsert = {
    debug: "",
    info: "",
    warn: chalk.yellow("! "),
    error: chalk.red("‼ "),
  }[line.level]

  if (options.ignoreInProduction && process.env["NODE_ENV"] === "production")
    return

  process.stdout.write(
    `${getPrefix(line.component)}${levelInsert}${line.message}${
      line.data
        ? " " +
          Object.entries(line.data)
            .map(([key, value]) => `${key}=${formatValue(value, options)}`)
            .join(", ")
        : ""
    }\n`
  )
}

export const createCliFormatter = (): Formatter => log
