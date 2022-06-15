import colors from "picocolors"

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

export default class CliFormatter {
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

    console.log(
      `${this.prefix}${levelInsert}${message}${
        data
          ? " " +
            Object.entries(data)
              .map(([key, value]) => `${key}=${value}`)
              .join(", ")
          : ""
      }`
    )
  }
}
