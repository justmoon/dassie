import colors from "picocolors"

import { inspect } from "node:util"

import type { Formatter } from "../types/formatter"
import type LogErrorOptions from "../types/log-error-options"
import isError from "../utils/is-error"

export const COLORS = [
  colors.red,
  colors.green,
  colors.yellow,
  colors.blue,
  colors.magenta,
  colors.cyan,
] as const

export const generateColoredPrefix = (seed: string) => {
  const hash =
    [...seed].reduce((hash, char) => hash + (char?.charCodeAt(0) ?? 0), 0) %
    COLORS.length

  return (COLORS[hash] ?? colors.black)(seed)
}

const monorepoRoot = new URL("../../../..", import.meta.url).pathname
export const formatFilePath = (filePath: string) => {
  let protocolPrefix = ""
  if (filePath.startsWith("file://")) {
    protocolPrefix = "file://"
    filePath = filePath.slice(7)
  }

  if (filePath.startsWith(monorepoRoot)) {
    const localPath = filePath.slice(monorepoRoot.length)

    const match = localPath.match(/^packages\/([a-z0-9-]+)\/(.*)$/) as [
      string,
      string,
      string
    ]
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
    return value()
  }

  return inspect(value)
}

export default class CliFormatter implements Formatter {
  readonly prefix: string

  constructor(readonly component: string) {
    this.prefix = generateColoredPrefix(component) + " "
  }

  clear() {
    console.log("\x1B[2J")
  }

  log(
    level: "debug" | "info" | "warn" | "error",
    message: string,
    data?: Record<string, unknown>
  ) {
    const levelInsert = {
      debug: "",
      info: "",
      warn: colors.yellow("! "),
      error: colors.red("‼ "),
    }[level]

    process.stdout.write(
      `${this.prefix}${levelInsert}${message}${
        data
          ? " " +
            Object.entries(data)
              .map(([key, value]) => `${key}=${formatValue(value)}`)
              .join(", ")
          : ""
      }\n`
    )
  }

  logError(error: unknown, options: LogErrorOptions = {}) {
    if (isError(error)) {
      const name = error.name ?? "Error"
      const message = error.message ?? ""

      let ignoreRest = false
      const stack = (error.stack?.split("\n").slice(1) ?? [])
        .map((line) => {
          if (ignoreRest) return null
          {
            const match = line.match(/^    at (.*) \((.*):(\d+):(\d+)\)$/) as
              | [string, string, string, string, string]
              | null
            if (match) {
              if (match[2].startsWith("node:")) {
                return colors.dim(line)
              }
              if (
                options.skipAfter &&
                match[1].indexOf(options.skipAfter) !== -1
              ) {
                ignoreRest = true
              }
              const isNodeModules = match[2].indexOf("node_modules") !== -1
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
            const match = line.match(/^    at (.*):(\d+):(\d+)$/) as
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
      this.log(
        "error",
        `${colors.red(colors.bold(`${name}:`))} ${colors.red(message)}${
          stack.length ? "\n" : ""
        }${stack.join("\n")}`
      )
    }
  }
}
