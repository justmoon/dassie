import colors from "picocolors"

import { inspect } from "node:util"

import type { Formatter } from "../types/formatter"
import type { LogLine } from "../types/log-line"
import type { LogErrorOptions } from "../types/log-options"
import isError from "../utils/is-error"
import { selectBySeed } from "../utils/select-by-seed"

export const COLORS = [
  colors.red,
  colors.green,
  colors.yellow,
  colors.blue,
  colors.magenta,
  colors.cyan,
] as const

const monorepoRoot = new URL("../../../..", import.meta.url).pathname
export const formatFilePath = (filePath: string) => {
  let protocolPrefix = ""
  if (filePath.startsWith("file://")) {
    protocolPrefix = "file://"
    filePath = filePath.slice(7)
  }

  if (filePath.startsWith(monorepoRoot)) {
    const localPath = filePath.slice(monorepoRoot.length)

    const match = localPath.match(/^packages\/([\da-z-]+)\/(.*)$/) as
      | [string, string, string]
      | null
    if (match) {
      return `${colors.dim(
        `${protocolPrefix}${monorepoRoot}packages/`
      )}${colors.cyan(match[1])}${colors.dim("/")}${match[2]}`
    }

    return `${colors.dim(`${protocolPrefix}${monorepoRoot}`)}${localPath}`
  }

  return filePath
}

export const formatValue = (value: unknown) => {
  if (typeof value === "function") {
    return String(value())
  }

  return inspect(value)
}

export default class CliFormatter implements Formatter {
  readonly prefixCache = new Map<string, string>()

  private getPrefix(component: string) {
    let prefix = this.prefixCache.get(component)
    if (!prefix) {
      prefix = selectBySeed(COLORS, component)(component) + " "
      this.prefixCache.set(component, prefix)
    }

    return prefix
  }

  clear() {
    console.log("\u001B[2J")
  }

  log(line: LogLine, options: LogErrorOptions) {
    const levelInsert = {
      debug: "",
      info: "",
      warn: colors.yellow("! "),
      error: colors.red("‼ "),
    }[line.level]

    if (options.ignoreInProduction && process.env["NODE_ENV"] === "production")
      return

    if (line.error) {
      line.message = this.formatError(line.error, options)
    }

    process.stdout.write(
      `${this.getPrefix(line.component)}${levelInsert}${line.message}${
        line.data
          ? " " +
            Object.entries(line.data)
              .map(([key, value]) => `${key}=${formatValue(value)}`)
              .join(", ")
          : ""
      }\n`
    )
  }

  formatError(error: unknown, options: LogErrorOptions = {}): string {
    if (isError(error)) {
      const name = error.name || "Error"
      const message = error.message

      let ignoreRest = false
      const stack = (error.stack?.split("\n").slice(1) ?? [])
        .map((line) => {
          if (ignoreRest) return null
          {
            const match = line.match(/^ {4}at (.*) \((.*):(\d+):(\d+)\)$/) as
              | [string, string, string, string, string]
              | null
            if (match) {
              if (match[2].startsWith("node:")) {
                return colors.dim(line)
              }
              if (options.skipAfter && match[1].includes(options.skipAfter)) {
                ignoreRest = true
              }
              const isNodeModules = match[2].includes("node_modules")
              const formattedFilePath = isNodeModules
                ? colors.dim(match[2])
                : formatFilePath(match[2])

              return `    ${colors.dim("at")} ${
                isNodeModules ? colors.dim(match[1]) : match[1]
              } ${colors.dim("(")}${formattedFilePath}${colors.dim(
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
              return `    ${colors.dim("at")} ${formattedFilePath}${colors.dim(
                `:${match[2]}:${match[3]}`
              )}`
            }
          }

          return colors.dim(line)
        })
        .filter(Boolean)

      return `${colors.red(colors.bold(`${name}:`))} ${colors.red(message)}${
        stack.length > 0 ? "\n" : ""
      }${stack.join("\n")}`
    } else {
      return `${colors.red(colors.bold("Thrown:"))} ${colors.red(
        formatValue(error)
      )}`
    }
  }
}
