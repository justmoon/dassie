import { getLogContext } from "./context"
import { createEnableChecker } from "./enabled"
import { createCliFormatter } from "./formatters/cli-formatter"

export * from "./common"

const cliFormatter = createCliFormatter()

export const context = getLogContext()

context.enableChecker = createEnableChecker(process.env["DEBUG"] ?? "")
context.output = (logEvent) => {
  process.stdout.write(cliFormatter(logEvent))
  process.stdout.write("\n")
}

const WORKING_DIRECTORY = `${process.cwd()}/`
context.getCaller = (depth, error) => {
  const stackLine = error.stack
    ?.split("\n")
    .filter((line) => line.includes("    at "))[depth]

  if (!stackLine) return undefined

  const filePathMatch = stackLine
    .match(/\((.*)\)$/)?.[1]
    ?.replace("file://", "")
    .replace(WORKING_DIRECTORY, "")

  return filePathMatch
}

context.captureCaller = import.meta.env.DEV
