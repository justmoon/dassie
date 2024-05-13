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
// In Node.js, stack frames with a name have the callsite in parentheses at
// the end. Stack frames without a name are just "    at " followed by the
// callsite.
const getCallsite = (stackLine: string) =>
  stackLine.endsWith(")") ?
    stackLine.match(/\((.*)\)$/)?.[1]
  : stackLine.match(/^\s*at\s+(.+)$/)?.[1]

context.getCaller = (depth, error) => {
  const stackLine = error.stack
    ?.split("\n")
    .filter((line) => line.includes("    at "))[depth]

  if (!stackLine) return undefined

  return getCallsite(stackLine)
    ?.replace("file://", "")
    .replace(WORKING_DIRECTORY, "")
}

context.captureCaller = import.meta.env.DEV
